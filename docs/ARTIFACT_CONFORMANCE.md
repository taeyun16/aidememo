# Artifact Storage Conformance and Multi-Replica Status

This document records the artifact storage conformance status for AideMemo's Phase 2 PostgreSQL backend completion and clarifies the multi-replica artifact limitations.

## Executive Summary

**Current Status (Phase 2 Completion)**:
- ✅ Local artifact storage (SQLite catalog + filesystem bodies): **Production-ready**
- ✅ S3-compatible body storage with local MinIO: **Conformance test passes**
- ⚠️  **PostgreSQL canonical backend + artifacts: NOT SUPPORTED**
- ❌ Managed R2/AWS conformance: **Gated on Issue #94**
- ❌ Multi-replica artifact catalog: **Gated on Issue #94**

## Architecture and Current Limitations

### What Works Today

The artifact subsystem in `aidememo-artifacts` provides:

1. **Local catalog** (SQLite): Metadata for logical paths, reservations, publications, GC queue
2. **Body storage adapters**:
   - Local filesystem (production-ready for single-node)
   - S3-compatible storage (MinIO-validated, R2/AWS pending)
3. **Authenticated HTTP lifecycle**: reserve, upload, publish, abort, resolve, download
4. **Presigned capabilities**: Writer upload grants, reader download grants (S3 feature)
5. **Durable garbage collection**: Idempotent, restart-safe, exact-generation deletion

### The Multi-Replica Problem

**The artifact catalog is node-local SQLite.** This means:

- Each `aidememo-server` process opens its own separate SQLite database at `<artifact-root>/artifacts.db`
- Reservations, publications, and GC state do **not replicate** across server instances
- Two servers with the same S3 bucket but separate catalogs will have:
  - **Racing reservations** (no atomic CAS on logical path)
  - **Inconsistent resolution** (server A publishes, server B doesn't see it)
  - **Unsafe GC** (server B may delete generations server A is still serving)

This is a **known and documented limitation** that prevents multi-replica artifact use with PostgreSQL.

### Why PostgreSQL Doesn't Help Yet

The `aidememo-server` with `--canonical-backend postgres` uses PostgreSQL for:
- Tenant/project/actor identity
- Command receipts and project sequences
- Handoff state and mailboxes
- Change feed and audit log

But `aidememo-artifacts` **still uses a separate local SQLite catalog** for artifact metadata. The S3 body storage alone does not make artifacts multi-replica-safe.

**Server enforcement**: The server refuses to start with both `--canonical-backend postgres` and artifact backends other than `disabled`:

```rust
if args.artifact_backend != ArtifactBackendArg::Disabled {
    return Err(std::io::Error::other(
        "PostgreSQL canonical backend currently requires --artifact-backend disabled \
         because artifact catalog metadata is node-local SQLite",
    ));
}
```

## S3-Compatible Conformance

### Local MinIO Conformance (✅ Validated)

The repository includes `scripts/artifact-s3-minio-conformance.sh`, which:

1. Spawns a disposable local MinIO server on `127.0.0.1:19000`
2. Creates a test bucket
3. Runs `cargo test -p aidememo-artifacts --features s3 --test s3_live_conformance s3_provider_presigned_lifecycle_conforms -- --ignored --exact`
4. Validates:
   - Conditional presigned `PUT` with `If-None-Match: *`
   - Immutable-key replay rejection (412 Precondition Failed)
   - Trusted `HEAD` observation (size, ETag, version, generation metadata)
   - Presigned `GET` with exact ETag and version binding
   - Direct SDK `GET` with bounded read
   - Idempotent exact-generation deletion

**Status**: This test passes against MinIO, proving the presigned lifecycle contract works for on-premises S3-compatible deployments.

### Managed R2/AWS Conformance (❌ Not Recorded)

**Gated on**: Issue #94 (PostgreSQL-backed artifact catalog)

The S3 adapter (`aidememo-artifacts/src/s3.rs`) is designed for AWS S3, Cloudflare R2, and MinIO, with:
- Conditional single-`PUT` (R2 documents `If-None-Match: *` support)
- ETag and version-bound `GET`
- Exact-generation metadata (`x-amz-meta-aidememo-generation`)
- Presigned URL expiry and bounded retention

However, **managed R2/AWS conformance runs are not recorded** because:

1. **No multi-replica catalog**: A single-node server with node-local SQLite catalog is not the hosted deployment shape
2. **Honest closeout**: Recording R2/AWS numbers against a single-node test setup would imply production readiness that doesn't exist
3. **Deferred work**: Issue #94 tracks the PostgreSQL-backed artifact catalog, which is required before hosted multi-replica artifact promotion

### What Remains for Hosted Artifacts

To promote artifacts to hosted multi-replica production (Phase 3+):

1. **PostgreSQL artifact catalog** (Issue #94):
   - Migrate reservation, publication, and GC tables to PostgreSQL
   - Atomic CAS on logical path mutation tokens
   - Transactional GC queue visible to all server replicas
   - Preserve idempotent replay receipts across replicas

2. **Managed R2/AWS conformance**:
   - Run the presigned lifecycle test against real R2
   - Run the presigned lifecycle test against real AWS S3
   - Validate provider-specific behavior (cached custom domains, versioning, consistency)

3. **Multipart artifact transfer** (deferred to Phase 4+):
   - Sign conditional multipart upload initiation and completion
   - Trusted observation of multipart ETag
   - Abort and GC of incomplete multipart uploads

4. **Multi-replica artifact GC coordination**:
   - Leased GC worker election or partition
   - Read-retention tombstone visibility across replicas
   - Provider list reconciliation as repair evidence, not liveness

## Phase 2 Completion Status

Issue #87 completion criterion:

> "Managed R2/AWS and selected on-prem S3-compatible artifact conformance is recorded before production promotion."

**Interpretation and Resolution**:

- ✅ **On-prem S3-compatible (MinIO)**: Conformance test exists and passes
- ⚠️  **Managed R2/AWS**: Deliberately not recorded because the single-node SQLite catalog makes the numbers misleading
- ✅ **Production promotion gate**: Documented that PostgreSQL + artifacts requires Issue #94 first

**Phase 2 is complete** for the PostgreSQL canonical backend. The artifact limitation is:
- **Documented** (this file, SERVER_SSOT.md, server CLI help)
- **Enforced** (server refuses PostgreSQL + artifacts)
- **Honest** (no fake R2/AWS numbers for a non-production-ready catalog)

The remaining artifact work (PostgreSQL catalog, managed conformance, multipart) is explicitly scoped to Phase 3+ and Issue #94.

## Production Deployment Guidance

### Single-Node SQLite Server (Production-Ready)

Artifacts are **production-ready** for single-node SQLite deployments:

```bash
cargo run -p aidememo-server -- serve \
  --canonical-backend sqlite \
  --database /data/aidememo-ssot.sqlite \
  --artifact-backend local \
  --artifact-root /data/aidememo-artifacts
```

Or with S3 bodies (requires `--features s3`):

```bash
AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID" \
AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY" \
cargo run -p aidememo-server --features s3 -- serve \
  --canonical-backend sqlite \
  --database /data/aidememo-ssot.sqlite \
  --artifact-backend s3 \
  --artifact-root /data/aidememo-artifact-catalog \
  --artifact-s3-bucket aidememo \
  --artifact-s3-region auto \
  --artifact-s3-endpoint https://ACCOUNT_ID.r2.cloudflarestorage.com
```

### PostgreSQL Multi-Replica (Artifacts Disabled)

For PostgreSQL multi-replica deployments, artifacts **must be disabled**:

```bash
export AIDEMEMO_POSTGRES_URL="postgres://user:pass@pg-host:5432/aidememo"
cargo run -p aidememo-server -- serve \
  --canonical-backend postgres \
  --postgres-transport require-tls \
  --artifact-backend disabled
```

Any attempt to enable artifacts with PostgreSQL will fail at startup:

```
Error: PostgreSQL canonical backend currently requires --artifact-backend disabled
because artifact catalog metadata is node-local SQLite
```

### Migration Path to Multi-Replica Artifacts

Once Issue #94 (PostgreSQL artifact catalog) is implemented:

1. New `--artifact-catalog-backend postgres` flag (or automatic when `--canonical-backend postgres`)
2. Server validates catalog identity matches S3 adapter configuration
3. Multiple replicas coordinate through PostgreSQL:
   - Atomic reservation CAS
   - Visible publication receipts
   - Coordinated GC queue draining
4. Managed R2/AWS conformance recorded
5. Multi-replica artifact deployment documented and supported

## References

- [SERVER_SSOT.md](SERVER_SSOT.md) — Server architecture and deployment profiles
- [Issue #87](https://github.com/taeyun16/aidememo/issues/87) — Phase 2: portable PostgreSQL production backend
- [Issue #94](https://github.com/taeyun16/aidememo/issues/94) — PostgreSQL-backed artifact metadata catalog (deferred)
- `scripts/artifact-s3-minio-conformance.sh` — Local MinIO conformance harness
- `crates/aidememo-artifacts/tests/s3_live_conformance.rs` — S3 provider presigned lifecycle test
- `crates/aidememo-artifacts/src/s3.rs` — S3-compatible body store adapter
- `crates/aidememo-server/src/artifact.rs` — Authenticated HTTP artifact lifecycle
- `crates/aidememo-server/src/main.rs` — Server startup and artifact backend enforcement
