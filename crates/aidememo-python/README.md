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

SQLite is the default local backend. To open redb stores, build the extension
with the Cargo `redb` feature and pass `backend="redb"`:

```bash
cd crates/aidememo-python
../../scripts/maturin.sh develop --release --features redb
```

```python
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
thread = g.fact_list(entity=pack["session_id"], limit=20)
```

For a multi-agent shared store, pass `source_id` on writes and reads. The same
field flows through `search`, `query`, `fact_list`, `fact_add`, `fact_add_many`,
and `workflow_start`.

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
| `search(query, limit?, min_confidence?)` | `list[dict]` |
| `query(topic, limit?, depth?, recent_limit?)` | `dict` |
| `workflow_start(title, body?, source?, source_id?, limit?, depth?, recent_limit?, bm25_only?)` | `dict` |
| `traverse(entity, depth?, direction?)` | `dict` |
| `path_find(from, to)` | `list[dict] \| None` |
| `entity_add(name, ...)` / `entity_get(name)` / `entity_list(...)` / `entity_delete(name)` | … |
| `resolve_entity(name)` | ULID string |
| `fact_add(content, ..., session_id?)` / `fact_add_many(items, session_id?)` / `fact_get(id)` / `fact_list(...)` / `fact_delete(id)` | … |
| `fact_pin(id, pinned)` / `pinned_facts(limit?)` | always-loaded facts |
| `relation_add/remove/get` | … |
| `ingest(wiki_root, incremental?)` | `dict` |
| `lint()` | `list[dict]` |
| `stats()` | `dict` |
