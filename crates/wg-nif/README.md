# wg_nif

Elixir bindings for [Wiki-Graph (`wg`)](https://github.com/taeyun16/wg).

## Install

Add to your `mix.exs`:

```elixir
def deps do
  [{:wg_nif, path: "../wg/crates/wg-nif"}]
end
```

The `:cargo` Mix compiler builds `target/{debug,release}/libwg_nif.{dylib,so}`
and copies it into `priv/libwg_nif.so` on every `mix compile` / `mix test`.
You only need the Rust toolchain installed.

## Quick start

```elixir
g = WgNif.open!("./_meta/wiki.redb")

ctx = WgNif.query(g, "Redis", limit: 5, depth: 2)
# %{"topic" => "Redis", "entity" => %{...}, "search" => [...], ...}

hits = WgNif.search(g, "high availability", limit: 10)
eid  = WgNif.entity_add(g, "Redis", entity_type: "technology")

WgNif.fact_add(g, "Redis Sentinel provides HA",
  entity_ids: [eid],
  fact_type: "decision",
  tags: ["ha"],
  confidence: 0.9
)
```

Read methods that return complex shapes (search, query, traverse, lint, …)
auto-decode JSON; you receive plain Elixir maps and lists. Write methods
return atoms or ULID strings.

## API surface

`WgNif` (high-level, JSON-decoding) wraps `WgNif.Native` (raw NIF). 21 functions:
`open!/1`, `version/0`, `search/3`, `query/3`, `traverse/3`, `path_find/3`,
`entity_add/3`, `entity_get/2`, `entity_list/2`, `entity_delete/2`,
`resolve_entity/2`, `fact_add/3`, `fact_get/2`, `fact_list/2`, `fact_delete/2`,
`relation_add/4`, `relation_remove/4`, `relations_get/3`, `ingest/3`,
`lint/1`, `stats/1`.

## Test

```bash
cd crates/wg-nif
mix test
```
