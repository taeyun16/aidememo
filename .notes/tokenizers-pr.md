# PR draft: huggingface/tokenizers — reduce Unigram loader heap by 39%

**Target**: `huggingface/tokenizers` (Rust crate, v0.22.2 / main)
**Files**: `tokenizers/src/models/unigram/{model.rs, trie.rs}`
**Patch**: `.notes/tokenizers-pr.patch`
**Author**: Taeyun Jang `<taeyun16@pm.me>`

## Title

`perf(unigram): pre-size token map and replace per-node HashMap with Vec`

## Body

While profiling `Unigram::from` for the 500 353-vocab
`minishlab/potion-multilingual-128M` tokenizer, two allocation sites
showed up as dominant in a `dhat` heap profile.

PR #1799 (v0.22.0, "Consolidated optimization ahash dary compact str")
already swapped the std `HashMap`s in this code path to
`ahash::AHashMap`, which addressed the *hasher* cost. The remaining
heap pressure is structural — the per-node `AHashMap` allocations
themselves and the missing capacity hint at the call site.

### Issue 1 — `token_to_ids: AHashMap::new()`

`models/unigram/model.rs:102` constructs `token_to_ids` with no
capacity hint, then immediately inserts `vocab.len()` entries:

```rust
let n = vocab.len();
let mut token_to_ids: TokenMap = AHashMap::new();
...
for (id, (token, score)) in vocab.iter().enumerate() {
    token_to_ids.insert(token.to_string(), id as u32);
    builder.push(token.as_bytes());
}
```

For a 500 k-vocab model that's ~17 doubling rehashes; each rehash
allocates a fresh table, copies every entry over, and frees the
old table. dhat attributes ~30 MB of total allocations to this
single site — all redundant since `n` is already in scope.

**Fix**: `AHashMap::with_capacity(n)`.

### Issue 2 — `Trie<Label>` `Node::children: AHashMap<Label, Node<Label>>`

`models/unigram/trie.rs` stores an `AHashMap` on every trie node.
For the 500 k-vocab tokenizer this materializes ~2.6 M
empty/near-empty hashbrown tables (one per node, plus per-entry
buckets). dhat shows this as the **single largest live-byte source
at peak**: ~210 MB across millions of small blocks on v0.22.2.

But trie nodes have very low fan-out — the only "wide" node is the
root (≤ alphabet size, in practice ≤ 256 for byte-level tries),
and interior nodes typically have 1–4 children. At those sizes a
linear scan over a packed `Vec<(Label, Node)>` beats a HashMap on
both axes:

| | AHashMap | Vec |
|---|---|---|
| Empty-node footprint | ~48 B (hashbrown header) | 24 B (Vec header, no heap alloc) |
| 4-entry node footprint | ~512 B (16-bucket table padded) | ~104 B (4 entries) |
| Lookup at fan-out=4 | hash + masking + branch | 3 byte compares |
| Lookup at fan-out=256 | hash + masking + branch | ≤256 byte compares (cache-resident, branch-predictable) |

**Fix**: `children: Vec<(Label, Node<Label>)>`, with linear scans in
`push` and `TrieIterator::next`. Public API of the trie is
unchanged.

## Measurement

Profiled against v0.22.2 baseline. Workload: a 1 500-fact wiki
search bench using `minishlab/potion-multilingual-128M`, decoding
20 queries and embedding each.

### dhat heap profile (release-with-debuginfo)

| | v0.22.2 baseline | both fixes applied | delta |
|---|---|---|---|
| Heap **peak** (`At t-gmax`) | 540.2 MB | **330.6 MB** | **-209 MB (-39%)** |
| Heap **total** (`Total`) | 1 161.7 MB | **903.3 MB** | **-259 MB (-22%)** |
| Blocks at gmax | 2 640 084 | 2 640 084 | same |

### Process-level (release build, peak_alloc + memory_stats)

Both columns measured on the same machine, same model, same workload:

| | v0.22.2 baseline | both fixes applied | delta |
|---|---|---|---|
| Process Heap peak | 515.2 MB | **315.3 MB** | -199.9 MB (-39%) |
| RSS peak | 840.8 MB | **554.0 MB** | -286.8 MB (-34%) |
| macOS `phys_footprint` peak | 708.3 MB | **421.4 MB** | -286.9 MB (-41%) |
| Total CPU user time | 1673.2 ms | **1447.0 ms** | -226 ms (-14%) |
| p50 search latency | 23.27 ms | 23.27 ms | same |
| p95 search latency | 59.59 ms | 59.32 ms | -0.3 ms (noise) |

The CPU win is incidental — fewer allocator round-trips through
the global allocator and tighter inner loops in the trie scan.

For comparison, the same two fixes applied on v0.20.4 (pre-ahash)
landed at gmax 300 MB / total 874 MB. ahash made the hasher faster
but the *per-node table overhead* persisted; the per-node Vec
switch is what addresses that.

## Test impact

`cargo test -p tokenizers --lib unigram` (20 tests) passes
unchanged on the patched build.

Encoding determinism verified externally: `cosine(encode_v0.22.2,
encode_patched) > 0.9999` on real text via Model2Vec round-trip.

## Compatibility

- Public API: unchanged.
- Behavior: identical (deterministic encode produces same vectors).
- Compile time: `Vec` is `Default`, no extra trait bounds needed.
- New deps: none. (`Trie::push` no longer needs `ahash::AHashMap`,
  but the rest of the file/tree still uses it; the `use ahash::…`
  import in `trie.rs` is removed since the trie itself no longer
  references it.)
