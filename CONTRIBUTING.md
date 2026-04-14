# Contributing

## Dev Setup
- Install Rust 1.85+
- `cargo build --workspace`
- `cargo test -p wg-core --lib`

## Adding features
- New CLI commands: `crates/wg-cli/src/cmd/`
- New core logic: `crates/wg-core/src/`
- New bindings: `crates/wg-{napi,python,nif,ffi}/`

## Testing
- Unit tests: `cargo test -p wg-core --lib`
- Integration: run CLI commands against `/tmp/test-wg`
