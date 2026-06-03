# Contributing to aidememo

Thanks for considering a contribution. The workflow is intentionally
minimal — fast iteration over heavy process.

## Setup

```bash
# Recommended: installs the same toolchain versions used by CI.
mise install
mise run ci-lint

# Rust-only fallback if you do not use mise.
rustup toolchain install 1.96.0 --component rustfmt --component clippy
# Workspace MSRV remains 1.85, but CI/development parity is pinned to 1.96.0.

# Recommended
brew install lefthook                    # or `npm i -g lefthook`
lefthook install                         # wires git hooks
```

Once `lefthook install` runs, every `git commit` and `git push` runs the
checks defined in `lefthook.yml`. Skip with `git commit --no-verify` when
the situation calls for it (very rare).

## Quality gates

| Where | Check | Fix |
|---|---|---|
| pre-commit | `cargo fmt --all --check` | `cargo fmt --all` |
| pre-commit | `cargo check --features semantic` | fix the errors |
| pre-commit (Elixir only) | `mix format --check-formatted` | `mix format` |
| pre-push | `cargo clippy --features semantic -- -D warnings` | fix or `#[allow(…)]` with reason |
| pre-push | `cargo doc --workspace --no-deps --features semantic` | fix rustdoc warnings |
| pre-push | `cargo test -p aidememo-core --features semantic` | fix the test |
| pre-push | `cargo test -p aidememo-cli --bin aidememo` | fix the test |
| CI | all of the above + `cargo doc -D warnings` + macOS test matrix | — |

Workspace lints (`Cargo.toml`) already deny `unwrap_used`, `panic`,
`dbg_macro` and `unsafe_code` (the `aidememo-ffi` and `aidememo-nif` crates opt out
of the `unsafe_code` deny because they intentionally bridge raw pointers).

## Testing

```bash
cargo test --workspace                              # default features
cargo test -p aidememo-core --features semantic           # includes hybrid search
cargo test -p aidememo-cli --bin aidememo                       # CLI parsing + helpers
./scripts/ci-local.sh lint
./scripts/ci-local.sh demo
./scripts/ci-local.sh test
./scripts/ci-local.sh
```

Bindings (Python / Node / Elixir / C) are not part of the default
`cargo test` flow because they need external toolchains. Run them
explicitly when touching binding code:

```bash
# Python
( cd crates/aidememo-python && maturin build --release \
  && pip install --user --force-reinstall ../../target/wheels/aidememo_python-*.whl \
  && python3 tests/smoke.py )

# Node
( cd crates/aidememo-napi && npm install && npm run build && node tests/smoke.js )

# Elixir
( cd crates/aidememo-nif && mix test )

# C-FFI
cargo build -p aidememo-ffi --release
cc crates/aidememo-ffi/example/smoke.c \
   -I crates/aidememo-ffi/include target/release/libaidememo_ffi.a \
   $LIBS -o /tmp/aidememo-ffi-smoke && /tmp/aidememo-ffi-smoke
```

`$LIBS` on macOS:
`-framework CoreFoundation -framework Security -framework SystemConfiguration`.
On Linux: `-lpthread -ldl -lm`.

## House rules

A few preferences captured from the codebase:

- **Field order matters in bpaf**: positional/command items must be the
  rightmost fields in the struct, and `construct!` argument order must
  match field order. `construct!` doesn't support `field: var` rename
  syntax — name the local variable the same as the field.
- **`AideMemo::*` write methods take `&self`** (interior mutability via
  `RwLock`). This makes `Arc<AideMemo>` callable from the bindings.
- **All persisted records are JSON, not bincode** — adding
  `#[serde(default)]` fields to types is fully backward-compatible with
  on-disk data. No migration needed.
- **No AI attribution in commit messages.** Imperative tense subject
  (`Add foo`, not `Added foo`). One-line summary, blank line, optional
  body. Don't add `Co-Authored-By: …` lines.

## Filing issues

Bug reports: include the OS, Rust version, the failing command, and the
last few lines of `cargo build 2>&1 | grep -E '^error'` if it's a
build issue.

Feature requests: describe the use case and the simplest API you'd want
to call. Bonus points for linking the equivalent feature in a competing
project — that gets us closer to design alignment.
