# aidememo-python

Python bindings for [AideMemo (`aidememo`)](https://github.com/taeyun16/aidememo) —
a local knowledge-graph wiki indexed with BM25 + semantic vectors.

## Install

```bash
pip install maturin
cd crates/aidememo-python
maturin develop --release   # builds + installs into the current Python env
```

Or build a wheel:

```bash
maturin build --release
pip install target/wheels/aidememo_python-*.whl
```

## Quick start

```python
import aidememo_python as aidememo

g = aidememo.AideMemo("./_meta/wiki.redb")

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

## Workflow start

Use `workflow_start` when an automation trigger only gives the agent a sparse
issue or ticket. It creates a tracked session, stores the trigger as a
`question` fact, and returns scoped decisions, lessons, errors, and search
context in one call.

```python
import aidememo_python as aidememo

g = aidememo.AideMemo("./team.redb")

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
```

For a multi-agent shared store, pass `source_id` on writes and reads. The same
field flows through `search`, `query`, `fact_list`, and `workflow_start`.

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
| `AideMemo(path)` | constructor |
| `search(query, limit?, min_confidence?)` | `list[dict]` |
| `query(topic, limit?, depth?, recent_limit?)` | `dict` |
| `workflow_start(title, body?, source?, source_id?, limit?, depth?, recent_limit?, bm25_only?)` | `dict` |
| `traverse(entity, depth?, direction?)` | `dict` |
| `path_find(from, to)` | `list[dict] \| None` |
| `entity_add(name, ...)` / `entity_get(name)` / `entity_list(...)` / `entity_delete(name)` | … |
| `resolve_entity(name)` | ULID string |
| `fact_add(content, ...)` / `fact_get(id)` / `fact_list(...)` / `fact_delete(id)` | … |
| `relation_add/remove/get` | … |
| `ingest(wiki_root, incremental?)` | `dict` |
| `lint()` | `list[dict]` |
| `stats()` | `dict` |
