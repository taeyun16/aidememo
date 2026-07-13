# aidememo_nif

Elixir bindings for [AideMemo (`aidememo`)](https://github.com/taeyun16/aidememo).

## Install

Add to your `mix.exs`:

```elixir
def deps do
  [{:aidememo_nif, path: "../aidememo/crates/aidememo-nif"}]
end
```

The `:cargo` Mix compiler builds `target/{debug,release}/libaidememo_nif.{dylib,so}`
and copies it into `priv/libaidememo_nif.so` on every `mix compile` / `mix test`.
You only need the Rust toolchain installed.

## Quick start

```elixir
g = AideMemoNif.open!("./_meta/wiki.sqlite")

ctx = AideMemoNif.query(g, "Redis", limit: 5, depth: 2)
# %{"topic" => "Redis", "entity" => %{...}, "search" => [...], ...}

hits = AideMemoNif.search(g, "high availability", limit: 10)
eid  = AideMemoNif.entity_add(g, "Redis", entity_type: "technology")

AideMemoNif.fact_add(g, "Redis Sentinel provides HA",
  entity_ids: [eid],
  fact_type: "decision",
  tags: ["ha"],
  source_id: "team-a",
  actor_id: "codex:account-a",
  confidence: 0.9
)

facts = AideMemoNif.fact_list(g, entity: "Redis", source_id: "team-a")
```

SQLite is the default local backend. Omit `backend` or pass an empty string to
use the compiled default. Pass `backend: "sqlite"` or `backend: "libsqlite"` to
select SQLite explicitly. To open redb stores, compile the NIF with the
Cargo `redb` feature and pass `backend: "redb"`:

```bash
cd crates/aidememo-nif
AIDEMEMO_NIF_CARGO_FEATURES=redb mix compile
```

```elixir
sqlite = AideMemoNif.open!("./_meta/wiki.sqlite", backend: "libsqlite")
g = AideMemoNif.open!("./_meta/wiki.redb", backend: "redb")
```

Read methods that return complex shapes (search, query, traverse, lint, …)
auto-decode JSON; you receive plain Elixir maps and lists. Write methods
return atoms or ULID strings.

## Shared source namespaces

For a multi-agent shared store, pass the same `source_id:` keyword to writes
and reads. The high-level module dispatches to the raw NIF's `_scoped`
functions for `search`, `query`, `traverse`, `path_find`, `entity_get` /
`entity_list`, `fact_get` / `fact_list`, `pinned_facts`, and `relation_add` /
`relations_get`. Omitting it preserves the legacy store-wide view.

```elixir
worker = AideMemoNif.entity_add(g, "Worker", entity_type: "service")
AideMemoNif.fact_add(g, "Worker uses Redis",
  entity_ids: [worker],
  source_id: "team-a",
  actor_id: "codex:account-a"
)
AideMemoNif.relation_add(g, "Redis", "Worker", "used_by", source_id: "team-a")

entity = AideMemoNif.entity_get(g, "Redis", source_id: "team-a")
edges = AideMemoNif.relations_get(g, "Redis",
  direction: "forward",
  source_id: "team-a"
)
path = AideMemoNif.path_find(g, "Redis", "Worker", source_id: "team-a")
always_on = AideMemoNif.pinned_facts(g, limit: 10, source_id: "team-a")
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

There is no handle-level source or actor default in `aidememo_nif`: pass
`source_id:` on every scoped operation and `actor_id:` on `fact_add` or each
`fact_add_many` item. `lint/1` and `stats/1` are always store-wide diagnostics;
do not expose them to source-restricted callers.

## Branch logs

The NIF exposes local branch-log push and merge through the already-open store
handle:

```elixir
candidate = AideMemoNif.open!("./candidate-b.sqlite", backend: "libsqlite")

push =
  AideMemoNif.branch_push(candidate, "candidate-b", "./shared",
    base: "./shared/backup-01..."
  )

main = AideMemoNif.open!("./main.sqlite", backend: "libsqlite")
merge = AideMemoNif.branch_merge(main, "./shared", branch: "candidate-b")
```

Use branch logs when several agents or experiments fork from the same backup and
you want to merge only the best result. Local paths are handled in-process. S3
branch logs should go through the `aidememo` CLI built with Cargo `--features
s3`.

## API surface

`AideMemoNif` (high-level, JSON-decoding) wraps `AideMemoNif.Native` (raw NIF).
Source-aware operations accept `source_id:` in their optional keyword list:
`search/3`, `query/3`, `traverse/3`, `path_find/4`, `entity_get/3`,
`entity_list/2`, `fact_get/3`, `fact_list/2`, `pinned_facts/2`,
`fact_add/3`, `relation_add/5`, and `relations_get/3`. `fact_add/3` also accepts
`actor_id:`; each `fact_add_many/2` item may carry its own `source_id` and
`actor_id`. Other core functions include
`open!/1`, `open!/2`, `version/0`, `entity_add/3`, `entity_delete/2`,
`entity_describe/3`, `resolve_entity/2`, `fact_add/3`, `fact_add_many/2`,
`fact_delete/2`, `fact_supersede/3`, `relation_remove/4`, `ingest/3`,
`lint/1`, `stats/1`, `branch_push/4`, and `branch_merge/3`.

## Test

```bash
cd crates/aidememo-nif
mix test
```
