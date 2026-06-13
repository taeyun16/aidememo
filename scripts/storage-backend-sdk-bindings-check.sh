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

run "${cargo_cmd[@]}" check -p aidememo-python
run "${cargo_cmd[@]}" test -p aidememo-napi
run "${cargo_cmd[@]}" check -p aidememo-nif
run "${cargo_cmd[@]}" test -p aidememo-ffi
run "${cargo_cmd[@]}" check -p aidememo-python --no-default-features --features redb
run "${cargo_cmd[@]}" test -p aidememo-napi --no-default-features --features redb
run "${cargo_cmd[@]}" check -p aidememo-nif --no-default-features --features redb
run "${cargo_cmd[@]}" test -p aidememo-ffi --no-default-features --features redb

if command -v mix >/dev/null 2>&1; then
    (
        cd "$ROOT_DIR/crates/aidememo-nif"
        run mix test
        run mix clean
        run env AIDEMEMO_NIF_CARGO_FEATURES=redb mix test
    )
else
    echo "skip: mix not found; cargo checked aidememo-nif default SQLite and redb feature builds"
fi

echo "storage backend SDK bindings check ok"
