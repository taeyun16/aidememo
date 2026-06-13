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

assert_redb_absent() {
    local label="$1"
    shift

    echo "==> cargo tree $* # redb must be absent ($label)"
    local tree
    tree="$("${cargo_cmd[@]}" tree "$@")"
    if rg '(^| )redb v[0-9]' <<<"$tree" >/dev/null; then
        echo "redb unexpectedly present in $label dependency tree" >&2
        rg '(^| )redb v[0-9]' <<<"$tree" >&2
        exit 1
    fi
}

assert_redb_present() {
    local label="$1"
    shift

    echo "==> cargo tree $* # redb must be present ($label)"
    local tree
    tree="$("${cargo_cmd[@]}" tree "$@")"
    if ! rg '(^| )redb v[0-9]' <<<"$tree" >/dev/null; then
        echo "redb missing from $label dependency tree" >&2
        exit 1
    fi
}

cd "$ROOT_DIR"

run "${cargo_cmd[@]}" check -p aidememo-core --no-default-features --features sqlite
run "${cargo_cmd[@]}" check -p aidememo-cli --no-default-features --features sqlite
run "${cargo_cmd[@]}" check -p aidememo-core --no-default-features --features s3
run "${cargo_cmd[@]}" check -p aidememo-core --no-default-features --features redb
run "${cargo_cmd[@]}" check -p aidememo-cli --no-default-features --features redb

assert_redb_absent "aidememo-core default" \
    -p aidememo-core
assert_redb_absent "aidememo-cli default" \
    -p aidememo-cli
assert_redb_absent "aidememo-python default" \
    -p aidememo-python
assert_redb_absent "aidememo-napi default" \
    -p aidememo-napi
assert_redb_absent "aidememo-nif default" \
    -p aidememo-nif
assert_redb_absent "aidememo-ffi default" \
    -p aidememo-ffi
assert_redb_absent "aidememo-core sqlite-only" \
    -p aidememo-core --no-default-features --features sqlite
assert_redb_absent "aidememo-cli sqlite-only" \
    -p aidememo-cli --no-default-features --features sqlite
assert_redb_absent "aidememo-python sqlite-only" \
    -p aidememo-python --no-default-features --features sqlite
assert_redb_absent "aidememo-napi sqlite-only" \
    -p aidememo-napi --no-default-features --features sqlite
assert_redb_absent "aidememo-nif sqlite-only" \
    -p aidememo-nif --no-default-features --features sqlite
assert_redb_absent "aidememo-ffi sqlite-only" \
    -p aidememo-ffi --no-default-features --features sqlite
assert_redb_absent "aidememo-core s3-only" \
    -p aidememo-core --no-default-features --features s3
assert_redb_present "aidememo-core redb feature" \
    -p aidememo-core --no-default-features --features redb
assert_redb_present "aidememo-cli redb feature" \
    -p aidememo-cli --no-default-features --features redb
assert_redb_present "aidememo-python redb feature" \
    -p aidememo-python --no-default-features --features redb
assert_redb_present "aidememo-napi redb feature" \
    -p aidememo-napi --no-default-features --features redb
assert_redb_present "aidememo-nif redb feature" \
    -p aidememo-nif --no-default-features --features redb
assert_redb_present "aidememo-ffi redb feature" \
    -p aidememo-ffi --no-default-features --features redb

echo "storage backend feature gate ok: default/SQLite/S3 omit redb, redb remains explicit across CLI and SDK crates"
