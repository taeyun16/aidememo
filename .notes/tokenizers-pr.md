# PR draft: huggingface/tokenizers — reduce loader heap on Unigram models

**Target**: `huggingface/tokenizers` (Rust crate)
**Files**: `tokenizers/src/models/unigram/{model.rs, trie.rs}`
**Patch**: `.notes/tokenizers-pr.patch`
**Author**: Taeyun Jang `<taeyun16@pm.me>`

## Title

`perf(unigram): pre-size token map and replace per-node HashMap with Vec`

## Body

While profiling the loader for `minishlab/potion-multilingual-128M` (a
500 353-vocab Unigram model) on macOS, two pieces of `Unigram::from`
showed up as dominant heap allocators with a `dhat` heap profile.

### Issue 1 — `token_to_ids: HashMap::new()`

`models/unigram/model.rs:101` constructs `token_to_ids` with no
capacity hint, then immediately inserts `vocab.len()` entries:

```rust
let n = vocab.len();
let mut token_to_ids: TokenMap = HashMap::new();
...
for (id, (token, score)) in vocab.iter().enumerate() {
    token_to_ids.insert(token.to_string(), id as u32);
}
```

For a 500 k-vocab model that's ~17 doubling rehashes; each rehash
allocates a fresh table, copies every entry over, and frees the
old table. dhat attributes ~34 MB of total allocations to this
single site — all redundant, since `n` is known up front.

**Fix**: `HashMap::with_capacity(n)`.

### Issue 2 — `Trie<Label>` `Node::children: HashMap<Label, Node<Label>>`

`models/unigram/trie.rs` stores a HashMap on every trie node. For
the 500 k-vocab tokenizer this materializes ~2.6 M empty/near-empty
hashbrown tables (one per node, plus per-entry buckets). dhat shows
this as the **single largest live-byte source at peak**: 307 MB
across 1.14 M blocks.

But trie nodes have very low fan-out — the only "wide" node is the
root (≤ alphabet size, in practice ≤ 256 for byte-level tries),
and interior nodes typically have 1–4 children. At those sizes a
linear scan over a packed `Vec<(Label, Node)>` beats a HashMap on
both axes:

| | HashMap | Vec |
|---|---|---|
| Empty-node footprint | ~48 B (hashbrown header) | 24 B (Vec header) |
| 4-entry node footprint | ~512 B (16-bucket table padded) | ~104 B (4 entries) |
| Lookup at fan-out=4 | hash + masking + branch | ~3 byte compares |
| Lookup at fan-out=256 | hash + masking + branch | ≤256 byte compares (cache-resident, branch-predictable) |

**Fix**: `children: Vec<(Label, Node<Label>)>`, with linear scans in
`push` and `TrieIterator::next`. Public API is unchanged.

## Measurement

End-to-end on a 1500-fact wiki search bench using
`minishlab/potion-multilingual-128M` (decode 20 queries, embed each):

| | upstream tokenizers 0.20.4 | both fixes applied |
|---|---|---|
| Heap **peak** (dhat `t-gmax`) | 432.6 MB | **300.0 MB** (−31%) |
| Heap **total** (dhat `Total`) | 1051.6 MB | **874.4 MB** (−17%) |
| Process RSS peak | 729 MB | **521 MB** (−29%) |
| macOS `phys_footprint` peak | 596 MB | **389 MB** (−35%) |
| Total CPU user time | 1695 ms | **1447 ms** (−15%) |
| p50 search latency | 23.4 ms | 23.5 ms (noise) |
| p95 search latency | 61.1 ms | 59.8 ms |

The CPU win is incidental — fewer allocator round-trips through
the global allocator, plus tighter inner loops in the trie scan.

## Test impact

`cargo test -p tokenizers --lib unigram` (20 tests) passes
unchanged on the patched build.

## Compatibility

- Public API: unchanged.
- Behavior: identical (deterministic encode produces same vectors;
  verified on real models).
- Compile-time: `Vec` is `Default`, no extra traits needed.
- New deps: none.
