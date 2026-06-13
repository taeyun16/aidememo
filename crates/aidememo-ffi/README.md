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

aidememo_store_t* g = aidememo_open("./_meta/wiki.redb");
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

To use the experimental local SQLite backend, build the library with the Cargo
`sqlite` feature and open with an explicit backend:

```bash
cargo build -p aidememo-ffi --features sqlite
```

```c
aidememo_store_t* g = aidememo_open_with_backend("./_meta/wiki.sqlite", "sqlite");
```

## API

All read functions return a heap-allocated, NUL-terminated UTF-8 JSON string.
**The caller MUST free it with `aidememo_free_string`.** On error, the JSON payload
is `{"error": "..."}` rather than NULL.

See `include/aidememo.h` for the complete signatures (~21 functions covering
search, query, traverse, path_find, entity/fact/relation CRUD, ingest,
lint, stats).

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
