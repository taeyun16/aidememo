#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLCHAIN="${AIDEMEMO_CI_TOOLCHAIN:-$(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT_DIR/rust-toolchain.toml" | head -n1)}"
BASE="${AIDEMEMO_STORAGE_FEATURE_GATE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-storage-feature-gate.XXXXXX")}"
BIN="$ROOT_DIR/target/debug/aidememo"

cleanup() {
    if [[ "${AIDEMEMO_STORAGE_FEATURE_GATE_KEEP_TMP:-0}" != "1" ]]; then
        rm -rf "$BASE"
    else
        echo "kept temp dir: $BASE" >&2
    fi
}
trap cleanup EXIT

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
    if grep -Eq '(^| )redb v[0-9]' <<<"$tree"; then
        echo "redb unexpectedly present in $label dependency tree" >&2
        grep -E '(^| )redb v[0-9]' <<<"$tree" >&2
        exit 1
    fi
}

assert_redb_present() {
    local label="$1"
    shift

    echo "==> cargo tree $* # redb must be present ($label)"
    local tree
    tree="$("${cargo_cmd[@]}" tree "$@")"
    if ! grep -Eq '(^| )redb v[0-9]' <<<"$tree"; then
        echo "redb missing from $label dependency tree" >&2
        exit 1
    fi
}

smoke_redb_only_default_store() {
    local home_dir="$BASE/redb-home"
    local work_dir="$BASE/redb-work"
    mkdir -p "$home_dir" "$work_dir"

    echo "==> redb-only default backend/path smoke"
    local backend
    backend="$(HOME="$home_dir" "$BIN" config get store.backend)"
    if [[ "$backend" != "redb" ]]; then
        echo "expected redb-only default backend to be redb, got $backend" >&2
        exit 1
    fi

    local store_path
    store_path="$(HOME="$home_dir" "$BIN" config get store.path)"
    if [[ "$store_path" != "./_meta/wiki.redb" ]]; then
        echo "expected redb-only default path to be ./_meta/wiki.redb, got $store_path" >&2
        exit 1
    fi

    (
        cd "$work_dir"
        HOME="$home_dir" "$BIN" --json stats >/dev/null
    )
    if [[ ! -f "$work_dir/_meta/wiki.redb" ]]; then
        echo "expected redb-only stats to create $work_dir/_meta/wiki.redb" >&2
        exit 1
    fi
    if [[ -f "$work_dir/_meta/wiki.sqlite" ]]; then
        echo "redb-only default smoke unexpectedly created $work_dir/_meta/wiki.sqlite" >&2
        exit 1
    fi
}

cd "$ROOT_DIR"

run "${cargo_cmd[@]}" check -p aidememo-core --no-default-features --features sqlite
run "${cargo_cmd[@]}" check -p aidememo-cli --no-default-features --features sqlite
run "${cargo_cmd[@]}" check -p aidememo-core --no-default-features --features s3
run "${cargo_cmd[@]}" check -p aidememo-core --no-default-features --features redb
run "${cargo_cmd[@]}" check -p aidememo-cli --no-default-features --features redb
run "${cargo_cmd[@]}" build -p aidememo-cli --no-default-features --features redb
smoke_redb_only_default_store

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

echo "storage backend feature gate ok: default/SQLite/S3 omit redb, redb remains explicit and redb-only defaults open a .redb store"
