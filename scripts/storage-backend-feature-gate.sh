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

assert_redb_absent "aidememo-core sqlite-only" \
    -p aidememo-core --no-default-features --features sqlite
assert_redb_absent "aidememo-cli sqlite-only" \
    -p aidememo-cli --no-default-features --features sqlite
assert_redb_absent "aidememo-core s3-only" \
    -p aidememo-core --no-default-features --features s3
assert_redb_present "aidememo-core redb feature" \
    -p aidememo-core --no-default-features --features redb
assert_redb_present "aidememo-cli redb feature" \
    -p aidememo-cli --no-default-features --features redb

echo "storage backend feature gate ok: SQLite/S3 omit redb, redb remains explicit"
