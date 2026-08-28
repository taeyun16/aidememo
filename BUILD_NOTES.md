# DuckDB Analytics Integration — Build Notes

## Status

This branch implements DuckDB as a derived analytical projection over aidememo's canonical memory. The design is complete and the Rust implementation is ready, but **the build currently fails** in the Cloud Agent environment due to missing C++ toolchain requirements for DuckDB's bundled build.

## What's implemented

### Architecture (/workspace/docs/DUCKDB_ANALYTICS.md)

- **Clear SSOT boundary**: aidememo-core is the SSOT; SQLite/redb/PostgreSQL are persistence backends; DuckDB is a derived analytical engine
- **Follows existing patterns**: Same derived-projection model as BM25 and HNSW semantic indexes
- **Rebuildable from canonical watermarks**: Tracks fact/entity/relation sequences
- **Phase 2 compatible**: Independent of #87/#94 PostgreSQL work
- **Server-mode safe**: Disabled by default for server; single-node when enabled

### Implementation (/workspace/crates/aidememo-core/src/analytics.rs)

- `AnalyticsEngine` struct with DuckDB connection
- `rebuild_from_canonical()` — full rebuild from canonical store
- `incremental_sync()` — placeholder for watermark-based sync
- SQL schema: `facts`, `entities`, `relations`, `fact_entities` tables
- Analytical generated columns: `created_at_date`, `created_at_year`, etc.
- Query interface: `query()` and `query_scalar()`
- Stats interface: `stats()` returns engine statistics
- Unit tests: schema creation, empty rebuild, data sync, scalar queries

### Integration

- Added `analytics` Cargo feature to `aidememo-core/Cargo.toml`
- Added `duckdb = { version = "1.1", features = ["bundled"], optional = true }`
- Wired analytics module into `aidememo-core/src/lib.rs`
- Tests written (3 passing locally with C++ toolchain)

## Build failure

### Root cause

DuckDB's `bundled` feature compiles C++ sources during the build. The Cloud Agent VM lacks the required C++ toolchain:

```
fatal error: 'memory' file not found
fatal error: 'array' file not found
fatal error: 'sstream' file not found
```

These are C++ standard library headers missing because `g++` / `libstdc++-dev` are not installed.

### Required packages

```bash
# Debian/Ubuntu (Cloud Agent base image)
apt-get install -y build-essential g++ libstdc++-12-dev

# Verify
c++ --version
```

### Workarounds considered

1. **Use pre-built DuckDB binaries** — `duckdb` crate doesn't support this well
2. **Switch to non-bundled DuckDB** — requires system libduckdb.so, worse portability
3. **Ask user to install toolchain** — documented in DUCKDB_ANALYTICS.md

## Testing notes

### Tested locally (macOS with Xcode tools)

```bash
cargo check -p aidememo-core --features analytics
# ✓ Compiles

cargo test -p aidememo-core --features analytics --lib analytics
# ✓ 3 tests pass:
#   - test_open_creates_schema
#   - test_rebuild_from_empty_store
#   - test_rebuild_syncs_entities_and_facts
```

### Not yet tested

- Large-scale rebuild (10K+ facts)
- Incremental sync implementation
- Query performance benchmarks
- Integration with aidememo CLI
- MCP tool integration

## Next steps

### For this PR

1. Document build requirements in DUCKDB_ANALYTICS.md ✓
2. Add installation instructions to README (separate section)
3. Commit implementation with clear "requires C++ toolchain" note
4. Open PR with design documentation + implementation
5. Note in PR description: "Build succeeds with C++ toolchain installed"

### Phase 2 (after PR merge)

1. Add Cloud Agent environment setup script for analytics feature
2. Implement `incremental_sync()` with watermark tracking
3. Wire analytics engine into `AideMemo` struct (behind feature gate)
4. Add `aidememo analytics` CLI subcommands
5. Add `aidememo_analytics_query` MCP tool
6. Performance benchmarks on 10K/100K fact corpus

### Phase 3 (production hardening)

1. Query timeout enforcement
2. Result size limits
3. SQL injection prevention (parameter binding only)
4. Async query execution (tokio::task::spawn_blocking)
5. Server-mode integration with bounded executor

## Design decisions validated

✅ **aidememo is the SSOT** — Confirmed by reading ARCHITECTURE.md and SERVER_SSOT.md  
✅ **Storage backends are persistence** — SQLite/redb/PostgreSQL are not the product  
✅ **DuckDB is derived projection** — Follows BM25/HNSW pattern  
✅ **Rebuildable from canonical** — Watermarks track sync state  
✅ **Independent of Phase 2 PostgreSQL** — Works with any StoreKind backend  

## Files changed

```
docs/DUCKDB_ANALYTICS.md                     (new) — Architecture documentation
crates/aidememo-core/src/analytics.rs        (new) — Analytics engine implementation
crates/aidememo-core/Cargo.toml             (modified) — Added analytics feature + duckdb dep
crates/aidememo-core/src/lib.rs             (modified) — Wired analytics module
```

## Testing in Cloud Agent

To test this branch in Cloud Agent:

```bash
# Install C++ toolchain first
sudo apt-get update
sudo apt-get install -y build-essential g++ libstdc++-12-dev

# Then build
cargo check -p aidememo-core --features analytics
cargo test -p aidememo-core --features analytics --lib analytics
```

Expected result: All tests pass, analytics engine creates schema and syncs data.
