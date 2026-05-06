# wg-python

Python bindings for [Wiki-Graph (`wg`)](https://github.com/taeyun16/wg) —
a local knowledge-graph wiki indexed with BM25 + semantic vectors.

## Install

```bash
pip install maturin
cd crates/wg-python
maturin develop --release   # builds + installs into the current Python env
```

Or build a wheel:

```bash
maturin build --release
pip install target/wheels/wg_python-*.whl
```

## Quick start

```python
import wg_python as wg

g = wg.WikiGraph("./_meta/wiki.redb")

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

## API

| Method | Returns |
|---|---|
| `WikiGraph(path)` | constructor |
| `search(query, limit?, min_confidence?)` | `list[dict]` |
| `query(topic, limit?, depth?, recent_limit?)` | `dict` |
| `traverse(entity, depth?, direction?)` | `dict` |
| `path_find(from, to)` | `list[dict] \| None` |
| `entity_add(name, ...)` / `entity_get(name)` / `entity_list(...)` / `entity_delete(name)` | … |
| `resolve_entity(name)` | ULID string |
| `fact_add(content, ...)` / `fact_get(id)` / `fact_list(...)` / `fact_delete(id)` | … |
| `relation_add/remove/get` | … |
| `ingest(wiki_root, incremental?)` | `dict` |
| `lint()` | `list[dict]` |
| `stats()` | `dict` |
