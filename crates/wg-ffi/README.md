# wg-ffi

C-ABI bindings for [Wiki-Graph (`wg`)](https://github.com/aspect-build/wg).

## Build

```bash
cargo build -p wg-ffi --release
# Outputs:
#   target/release/libwg_ffi.a       (staticlib — link directly)
#   target/release/libwg_ffi.dylib   (cdylib — runtime load)
```

## Use from C

```c
#include "wg.h"

wg_store_t* g = wg_open("./_meta/wiki.redb");
char* json = wg_query(g, "Redis", 5, 2, 5);
printf("%s\n", json);
wg_free_string(json);
wg_close(g);
```

Compile against the staticlib (recommended — no `LD_LIBRARY_PATH` needed):

```bash
cc your.c \
   -I crates/wg-ffi/include \
   target/release/libwg_ffi.a \
   -framework CoreFoundation -framework Security -framework SystemConfiguration \
   -o your-bin
```

(On Linux: drop the `-framework` flags; add `-lpthread -ldl -lm` if your linker needs them.)

## API

All read functions return a heap-allocated, NUL-terminated UTF-8 JSON string.
**The caller MUST free it with `wg_free_string`.** On error, the JSON payload
is `{"error": "..."}` rather than NULL.

See `include/wg.h` for the complete signatures (~21 functions covering
search, query, traverse, path_find, entity/fact/relation CRUD, ingest,
lint, stats).

## Smoke test

```bash
cargo build -p wg-ffi --release
cc crates/wg-ffi/example/smoke.c \
   -I crates/wg-ffi/include \
   target/release/libwg_ffi.a \
   -framework CoreFoundation -framework Security -framework SystemConfiguration \
   -o target/wg-ffi-smoke
target/wg-ffi-smoke
```

## Thread safety

A single `wg_store_t*` is safe to share across threads — the underlying
graph uses an `RwLock` internally.
