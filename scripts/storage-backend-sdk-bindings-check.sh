#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLCHAIN="${AIDEMEMO_CI_TOOLCHAIN:-$(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT_DIR/rust-toolchain.toml" | head -n1)}"

cargo_cmd=(cargo)
if [[ -n "${TOOLCHAIN}" ]]; then
    cargo_cmd+=("+${TOOLCHAIN}")
fi

run() {
    echo "==> $*"
    "$@"
}

run "${cargo_cmd[@]}" check -p aidememo-python --features sqlite
run "${cargo_cmd[@]}" test -p aidememo-napi --features sqlite
run "${cargo_cmd[@]}" check -p aidememo-nif --features sqlite
run "${cargo_cmd[@]}" test -p aidememo-ffi --features sqlite

if command -v mix >/dev/null 2>&1; then
    (
        cd "$ROOT_DIR/crates/aidememo-nif"
        run env AIDEMEMO_NIF_CARGO_FEATURES=sqlite mix test
    )
else
    echo "skip: mix not found; cargo checked aidememo-nif --features sqlite"
fi

echo "storage backend SDK bindings check ok"
