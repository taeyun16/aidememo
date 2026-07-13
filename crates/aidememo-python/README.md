# aidememo-python

Python bindings for [AideMemo (`aidememo`)](https://github.com/taeyun16/aidememo) —
a local knowledge-graph wiki indexed with BM25 + semantic vectors.

## Install

From a checkout, use the pinned release toolchain:

```bash
mise install
mise run python-pack-smoke
```

For iterative local development, install into the current Python environment:

```bash
cd crates/aidememo-python
../../scripts/maturin.sh develop --release
```

Or build a wheel:

```bash
cd crates/aidememo-python
../../scripts/maturin.sh build --release
pip install target/wheels/aidememo_python-*.whl
```

After the public PyPI release:

```bash
python -m pip install aidememo-python
```

The wrapper runs `maturin` through the pinned `uvx` spec in `mise.toml`; no
global `maturin` install is required.

## Quick start

```python
import aidememo_python as aidememo

g = aidememo.AideMemo("./_meta/wiki.sqlite")

# Unified context fetch
ctx = g.query("Redis", limit=5, depth=2, recent_limit=5)
print(ctx["entity"], ctx["search"], ctx["related"])

# Hybrid search
hits = g.search("high availability", limit=10)

# Graph traversal
result = g.traverse("Redis", depth=2, direction="both")

# Add a fact
fact_id = g.fact_add(
    "Redis Sentinel provides high availability",
    entity_ids=[g.resolve_entity("Redis")],
    fact_type="decision",
    tags=["ha"],
)

# Ingest a markdown wiki
stats = g.ingest("./my-wiki", incremental=False)
print(stats)  # {entities_added, facts_added, ...}
```

SQLite is the default local backend. Omit `backend` or pass an empty string to
use the compiled default. Pass `backend="sqlite"` or `backend="libsqlite"` to
select SQLite explicitly. To open redb stores, build the extension with the
Cargo `redb` feature and pass `backend="redb"`:

```bash
cd crates/aidememo-python
../../scripts/maturin.sh develop --release --features redb
```

```python
g = aidememo.AideMemo("./_meta/wiki.sqlite", backend="libsqlite")
g = aidememo.AideMemo("./_meta/wiki.redb", backend="redb")
```

## Workflow start

Use `workflow_start` when an automation trigger only gives the agent a sparse
issue or ticket. It creates a tracked session, stores the trigger as a
`question` fact, and returns scoped decisions, lessons, errors, and search
context in one call.

```python
import aidememo_python as aidememo

g = aidememo.AideMemo("./team.sqlite")

redis = g.entity_add("Redis", entity_type="technology")
g.fact_add(
    "Decision: Redis worker jobs must wrap DNS timeouts with retries",
    entity_ids=[redis],
    fact_type="decision",
    source_id="team-a",
)
g.fact_add(
    "Lesson: Redis timeout incidents were hard to debug without DNS metrics",
    entity_ids=[redis],
    fact_type="lesson",
    source_id="team-a",
)

pack = g.workflow_start(
    "Fix Redis timeout in worker",
    body="Worker jobs intermittently time out. The issue has no more detail.",
    source="github:org/app#123",
    source_id="team-a",
    limit=8,
    depth=2,
    recent_limit=5,
    bm25_only=True,  # keep cold-start deterministic in hooks/tests
)

print(pack["session_id"])
print(pack["ticket_fact_id"])
print([hit["content"] for hit in pack["relevant_decisions"]])

g.fact_add(
    "Lesson: follow-up facts can attach to this workflow session",
    entity_ids=[redis],
    fact_type="lesson",
    source_id="team-a",
    session_id=pack["session_id"],
)
thread = g.fact_list(entity=pack["session_id"], source_id="team-a", limit=20)
```

## Shared source namespaces

For a multi-agent shared store, pass the same `source_id` on writes and reads.
It scopes `search`, `query`, `workflow_start`, `traverse`, `path_find`,
`entity_get` / `entity_list`, `fact_get` / `fact_list`, `fact_pin` /
`pinned_facts`, and `relation_add` / `relations_get`. `workflow_start` carries
the namespace into its ticket fact, optional parent-session relation, and all
returned retrieval context. Omitting `source_id` preserves the legacy
store-wide view.

```python
worker = g.entity_add("Worker", entity_type="service")
g.fact_add("Worker uses Redis", entity_ids=[worker], source_id="team-a")
g.relation_add("Redis", "Worker", "used_by", source_id="team-a")

entity = g.entity_get("Redis", source_id="team-a")
facts = g.fact_list(entity="Redis", source_id="team-a")
edges = g.relations_get("Redis", direction="forward", source_id="team-a")
path = g.path_find("Redis", "Worker", source_id="team-a")
g.fact_pin(facts[0]["id"], True, source_id="team-a")
always_on = g.pinned_facts(limit=10, source_id="team-a")
```

Exact-content deduplication is local to a source namespace, so two sources can
store the same text as independent facts with distinct provenance.
Entities are a shared ontology: names, IDs, and types can be reused by several
sources. A scoped entity read only exposes entities backed by facts in that
source and omits globally-authored descriptive metadata; scoped relation reads
return only edges added with that exact `source_id`. The native binding does
not authenticate callers or prevent them from choosing another `source_id`,
and global mutation methods remain available. Treat this as a trusted-team
boundary. Use separate stores/processes for untrusted tenants, or expose the
store through the MCP server's token-to-source bindings described in
[`docs/MCP.md`](../../docs/MCP.md).

## Branch logs

Use `branch_push` / `branch_merge` when a Python agent or plugin forks a memory
store for speculative work and wants to merge only the winning branch.

```python
candidate = aidememo.AideMemo("./candidate-b.sqlite", backend="libsqlite")
candidate.branch_push(
    "candidate-b",
    "./shared",
    base="./shared/backup-01...",
)

main = aidememo.AideMemo("./main.sqlite", backend="libsqlite")
main.branch_merge("./shared", branch="candidate-b")
```

Local branch paths use the already-open native store handle, so SDK/plugin code
does not reopen the same database file. S3 branch URIs should use the CLI
`aidememo branch ...` commands from a build compiled with `--features s3`.

## Errors

Core `aidememo` failures are mapped to typed Python exceptions. Every message starts
with a stable machine-readable code such as `[entity_not_found]`.

```python
try:
    g.entity_get("Rdis")
except aidememo.AideMemoNotFoundError as exc:
    print(exc)  # [entity_not_found] entity not found: 'Rdis' ...
except aidememo.AideMemoError as exc:
    print("aidememo failed", exc)
```

Exception classes:

| Exception | Use |
|---|---|
| `AideMemoError` | Base class for all aidememo-python errors |
| `AideMemoNotFoundError` | Missing entity, fact, relation, or path |
| `AideMemoInvalidInputError` | Invalid caller input or schema mismatch |
| `AideMemoStoreError` | Store open/read/write/config IO failures |
| `AideMemoSearchError` | Search, index, or embedding-model failures |

## API

| Method | Returns |
|---|---|
| `AideMemo(path, backend?, durability?, model?, semantic_index?)` | constructor |
| `search(query, ..., source_id?)` | `list[dict]` |
| `query(topic, ..., source_id?)` | `dict` |
| `workflow_start(title, body?, source?, source_id?, actor_id?, parent_session_id?, ...)` | `dict` |
| `traverse(entity, depth?, direction?, source_id?)` | `dict` |
| `path_find(from, to, source_id?)` | `list[dict] \| None` |
| `entity_add(name, ...)` / `entity_get(name, source_id?)` / `entity_list(..., source_id?)` / `entity_delete(name)` | … |
| `resolve_entity(name)` | ULID string |
| `fact_add(content, ..., source_id?, actor_id?, session_id?)` / `fact_add_many(items, session_id?)` | ULID(s) |
| `fact_get(id, source_id?)` / `fact_list(..., source_id?)` / `fact_delete(id)` | … |
| `fact_pin(id, pinned, source_id?)` / `pinned_facts(limit?, source_id?)` | always-loaded facts |
| `relation_add(source, target, type, source_id?)` / `relations_get(entity, direction?, source_id?)` / `relation_remove(...)` | … |
| `ingest(wiki_root, incremental?)` | `dict` |
| `lint()` | `list[dict]` |
| `stats()` | `dict` |
| `branch_push(branch, destination, base?)` / `branch_merge(source, branch?)` | `dict` |
