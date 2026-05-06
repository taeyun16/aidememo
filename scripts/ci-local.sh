#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLCHAIN="${WG_CI_TOOLCHAIN:-$(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT_DIR/rust-toolchain.toml" | head -n1)}"

cargo_cmd=(cargo)
if [[ -n "${TOOLCHAIN}" ]]; then
    cargo_cmd+=("+${TOOLCHAIN}")
fi

run() {
    echo "→ $*"
    "$@"
}

lint() {
    run "${cargo_cmd[@]}" fmt --all -- --check
    run "${cargo_cmd[@]}" clippy --workspace --all-targets --features semantic -- -D warnings
    run env RUSTDOCFLAGS=-D\ warnings "${cargo_cmd[@]}" doc --workspace --no-deps --features semantic
}

tests() {
    run "${cargo_cmd[@]}" test --workspace --no-default-features
    run "${cargo_cmd[@]}" test -p wg-core --features semantic
    run "${cargo_cmd[@]}" test -p wg-cli --bin wg
}

case "${1:-all}" in
    lint)
        lint
        ;;
    test)
        tests
        ;;
    all)
        lint
        tests
        ;;
    *)
        echo "usage: $0 [lint|test|all]" >&2
        exit 1
        ;;
esac
