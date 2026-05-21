#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_NPM="${WG_BINDINGS_SMOKE_NPM:-1}"
RUN_OPTIONAL="${WG_BINDINGS_SMOKE_OPTIONAL:-0}"

run() {
    echo "==> $*"
    "$@"
}

have() {
    command -v "$1" >/dev/null 2>&1
}

status_line() {
    printf '%-14s %-8s %s\n' "$1" "$2" "$3"
}

cd "$ROOT_DIR"

echo "binding release smoke"
echo
status_line "binding" "status" "detail"
status_line "-------" "------" "------"

run cargo check -p wg-python -p wg-napi -p wg-nif -p wg-ffi

if [[ "$RUN_NPM" == "1" ]]; then
    run scripts/wg-napi-version.sh
    run scripts/wg-napi-pack-smoke.sh
    status_line "wg-napi" "ok" "version gate + root/platform pack/install smoke"
else
    status_line "wg-napi" "skip" "set WG_BINDINGS_SMOKE_NPM=1 to run npm pack/install smoke"
fi

if have maturin; then
    if [[ "$RUN_OPTIONAL" == "1" ]]; then
        run bash -lc 'cd crates/wg-python && maturin build --release -o /tmp/wg-python-wheel'
        status_line "wg-python" "ok" "maturin build --release"
    else
        status_line "wg-python" "ready" "maturin found; set WG_BINDINGS_SMOKE_OPTIONAL=1 to build wheel"
    fi
else
    status_line "wg-python" "todo" "install maturin, then run: cd crates/wg-python && maturin build --release"
fi

if have mix; then
    if [[ "$RUN_OPTIONAL" == "1" ]]; then
        run bash -lc 'cd crates/wg-nif && mix deps.get && mix compile.cargo --force && mix test'
        status_line "wg-nif" "ok" "mix compile.cargo --force && mix test"
    else
        status_line "wg-nif" "ready" "mix found; set WG_BINDINGS_SMOKE_OPTIONAL=1 to run mix test"
    fi
else
    status_line "wg-nif" "todo" "install Elixir/Mix, then run: cd crates/wg-nif && mix deps.get && mix compile.cargo --force && mix test"
fi

if have cc; then
    if [[ "$RUN_OPTIONAL" == "1" ]]; then
        run cargo build -p wg-ffi
        run cc crates/wg-ffi/example/smoke.c -I crates/wg-ffi/include -L target/debug -lwg_ffi -o target/wg-ffi-smoke
        case "$(uname -s)" in
            Darwin)
                run env DYLD_LIBRARY_PATH="$ROOT_DIR/target/debug:${DYLD_LIBRARY_PATH:-}" target/wg-ffi-smoke
                ;;
            Linux)
                run env LD_LIBRARY_PATH="$ROOT_DIR/target/debug:${LD_LIBRARY_PATH:-}" target/wg-ffi-smoke
                ;;
            *)
                run target/wg-ffi-smoke
                ;;
        esac
        status_line "wg-ffi" "ok" "C smoke linked against target/debug/libwg_ffi"
    else
        status_line "wg-ffi" "ready" "cc found; set WG_BINDINGS_SMOKE_OPTIONAL=1 to run C smoke"
    fi
else
    status_line "wg-ffi" "todo" "install a C compiler, then run the README smoke"
fi

echo
echo "OK: binding release smoke completed"
