# Vendored Rust Dependency

`tokenizers/` is a focused fork of Hugging Face `tokenizers` 0.22.2 used by the
optional fastembed provider. It retains two allocation reductions in the
Unigram loader: pre-sizing the token map and storing low-fanout trie children
in packed vectors. The default Model2Vec provider depends on registry
`tokenizers` 0.21.4 and does not compile this fork.

The repository-root `vendor/tokenizers` symlink points here so Cargo's
`[patch.crates-io]` path and the Python sdist use the same source. Keep the
crate's source, license, focused tests, and benchmarks. Unrelated upstream demo
applications, generated frontend locks, and build outputs are intentionally
omitted.
