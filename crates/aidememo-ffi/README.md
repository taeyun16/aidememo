# aidememo-ffi

C-ABI bindings for [AideMemo (`aidememo`)](https://github.com/taeyun16/aidememo).

## Build

```bash
cargo build -p aidememo-ffi --release
# Outputs:
#   target/release/libaidememo_ffi.a       (staticlib — link directly)
#   target/release/libaidememo_ffi.dylib   (cdylib — runtime load)
```

## Use from C

```c
#include "aidememo.h"

aidememo_store_t* g = aidememo_open("./_meta/wiki.sqlite");
char* json = aidememo_query(g, "Redis", 5, 2, 5);
printf("%s\n", json);
aidememo_free_string(json);
aidememo_close(g);
```

Compile against the staticlib (recommended — no `LD_LIBRARY_PATH` needed):

```bash
cc your.c \
   -I crates/aidememo-ffi/include \
   target/release/libaidememo_ffi.a \
   -framework CoreFoundation -framework Security -framework SystemConfiguration \
   -o your-bin
```

(On Linux: drop the `-framework` flags; add `-lpthread -ldl -lm` if your linker needs them.)

SQLite is the default local backend. Pass `NULL` / `""` to use the compiled
default, or pass `"sqlite"` / `"libsqlite"` to select SQLite explicitly. To
open redb stores, build the library with the Cargo `redb` feature and open with
an explicit backend:

```bash
cargo build -p aidememo-ffi --features redb
```

```c
aidememo_store_t* sqlite = aidememo_open_with_backend("./_meta/wiki.sqlite", "libsqlite");
aidememo_store_t* g = aidememo_open_with_backend("./_meta/wiki.redb", "redb");
```

## Shared source namespaces

The legacy functions keep the store-wide API. Their `_scoped` counterparts
accept a `source_id` for shared-agent stores:

- `aidememo_search_scoped` / `aidememo_query_scoped`
- `aidememo_traverse_scoped` / `aidememo_path_find_scoped`
- `aidememo_entity_get_scoped` / `aidememo_entity_list_scoped`
- `aidememo_fact_get_scoped` / `aidememo_fact_list_scoped`
- `aidememo_fact_pin_scoped` / `aidememo_pinned_facts_scoped`
- `aidememo_relation_add_scoped` / `aidememo_relations_get_scoped`

Use `aidememo_fact_add_scoped` (or `source_id` in each
`aidememo_fact_add_many` item) for writes, then use the same namespace for
every read:

```c
char* fact = aidememo_fact_add_scoped(
    g, "Redis Sentinel provides HA", ids_json, "decision", NULL, NULL, 1.0f,
    "team-a", "worker-1");
aidememo_free_string(fact);

char* facts = aidememo_fact_list_scoped(
    g, "Redis", NULL, 10, true, "team-a");
printf("%s\n", facts);
aidememo_free_string(facts);
```

The C handle has no source or actor default: pass `source_id` to every scoped
call. `aidememo_fact_add_scoped` accepts `actor_id`, and each
`aidememo_fact_add_many` item may carry independent `source_id` and `actor_id`
values.

Exact-content deduplication is local to a source namespace, so two sources can
store the same text as independent facts with distinct provenance.
Entities are a shared ontology: names, IDs, and types can be reused by several
sources. A scoped entity read only exposes entities backed by facts in that
source and omits globally-authored descriptive metadata; scoped relation reads
return only edges added with that exact `source_id`. The C ABI does not
authenticate callers or prevent them from choosing another `source_id`, and
global mutation functions remain available. Treat this as a trusted-team
boundary. Use separate stores/processes for untrusted tenants, or expose the
store through the MCP server's token-to-source bindings described in
[`docs/MCP.md`](../../docs/MCP.md).

`aidememo_lint` and `aidememo_stats` always report store-wide diagnostics; do
not expose them to source-restricted callers.

## API

All read functions return a heap-allocated, NUL-terminated UTF-8 JSON string.
**The caller MUST free it with `aidememo_free_string`.** On error, the JSON payload
is `{"error": "..."}` rather than NULL.

See `include/aidememo.h` for the complete signatures covering store-wide and
source-scoped search, query, graph traversal, entity/fact/relation CRUD,
ingest, lint, and stats.

## Smoke test

```bash
cargo build -p aidememo-ffi --release
cc crates/aidememo-ffi/example/smoke.c \
   -I crates/aidememo-ffi/include \
   target/release/libaidememo_ffi.a \
   -framework CoreFoundation -framework Security -framework SystemConfiguration \
   -o target/aidememo-ffi-smoke
target/aidememo-ffi-smoke
```

## Thread safety

A single `aidememo_store_t*` is safe to share across threads — the underlying
graph uses an `RwLock` internally.
