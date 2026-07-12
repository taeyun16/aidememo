# Contributing to AideMemo

Thanks for considering a contribution. The workflow is intentionally
minimal — fast iteration over heavy process.

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
For usage questions and design discussion, use
[GitHub Discussions](https://github.com/taeyun16/aidememo/discussions). Use the
structured issue forms for reproducible bugs and scoped feature requests. Report
security issues privately as described in [SECURITY.md](SECURITY.md).

## Contribution workflow

1. Search existing issues and discussions. Open an issue first for substantial,
   breaking, or cross-surface changes.
2. Fork the repository and create a focused branch from `main`.
3. Keep each pull request small enough to review and include tests or concrete
   verification evidence.
4. Update public docs, examples, changelog notes, and Korean translations when
   the affected surface requires them.
5. Open a pull request, link the issue, and explain any skipped checks.

All contributions are licensed under the repository's dual MIT OR Apache-2.0
terms. Do not submit code, fixtures, model output, or benchmark data that you do
not have permission to redistribute.

## Setup

```bash
# Recommended: installs the same toolchain versions used by CI.
mise install
mise run ci-lint

# Rust-only fallback if you do not use mise.
rustup toolchain install 1.96.0 --component rustfmt --component clippy
# Workspace MSRV is 1.95; CI/development parity is pinned to 1.96.0.

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
| pre-commit | `cargo fmt --all -- --check` | `cargo fmt --all` |
| pre-commit | `cargo check --features semantic` | fix the errors |
| pre-commit (Elixir only) | `mix format --check-formatted` | `mix format` |
| pre-push | `cargo clippy --features semantic -- -D warnings` | fix or `#[allow(…)]` with reason |
| pre-push | `cargo doc --workspace --no-deps --features semantic` | fix rustdoc warnings |
| pre-push | `cargo test -p aidememo-core --features semantic` | fix the test |
| pre-push | `cargo test -p aidememo-cli --bin aidememo` | fix the test |
| CI / release | `python3 scripts/public-portability-check.py` | replace developer-specific home paths with repository-relative, `$HOME`, or environment-driven paths |
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

## Documentation and localization

English documentation under `docs/` is the source locale. Korean Markdown
translations live under
`website/i18n/ko/docusaurus-plugin-content-docs/current/`. The translated and
intentional English-fallback sets are recorded in
`website/i18n/ko/translation-status.json`.

When changing a translated English document:

1. Update the matching Korean Markdown file in the same change.
2. Run `mise run docs-i18n-update` after reviewing the translation. This
   records the new source SHA-256; it is not a substitute for translation
   review.
3. Run `python3 scripts/docs-feature-gate.py` and
   `python3 scripts/docs-site-e2e.py` to validate both locales.

When changing homepage, navbar, footer, or sidebar text, run
`npm --prefix website run write-translations:ko` and review the resulting JSON
diff before restoring the Korean messages. Preview one locale at a time with
`mise run docs-start` or `mise run docs-start-ko`; a production
`mise run docs-build` includes both locales.

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

## Review and release expectations

- Maintainers may ask to split unrelated changes or add focused regression tests.
- CI must pass before merge; platform-specific checks may be required for native bindings.
- Breaking changes need an explicit migration path and release-note entry.
- Registry releases are maintainer-operated through protected OIDC workflows.
