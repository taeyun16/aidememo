# model2vec-native

Zero-copy, mmap-backed [Model2Vec](https://github.com/MinishLab/model2vec)
inference for Rust. Drop-in API match for `model2vec-rs`, but with the
489 MB embedding matrix kept file-backed instead of copied twice into
the heap on load.

## What this does differently

`model2vec-rs` (the upstream Rust port) loads `model.safetensors` with
`fs::read` (one full Vec<u8> copy) and then re-decodes the F32
little-endian payload into a fresh `Vec<f32>` via
`chunks_exact(4).map(...).collect()` (a second copy). For the default
128M-parameter `potion-multilingual-128M` model that's two ~500 MB
heap allocations live concurrently during load; the `Vec<f32>` then
stays resident.

This crate eliminates both copies:

- **mmap** the weights file. No heap allocation for the matrix —
  the OS pages it in lazily and can evict cold pages under pressure.
- **`bytemuck::cast_slice`** the mmap'd region to `&[f32]`. SafeTensors
  payloads are little-endian f32; on every Rust target that's the
  same wire format as `f32`, so the cast is a pointer reinterpret
  with zero allocation.

Net effect on the default 128M model:

| | model2vec-rs | model2vec-native |
|---|---:|---:|
| Heap peak (load) | ~1.5 GB | ~410 MB |
| RSS peak | ~1.8 GB | ~730 MB |
| First-encode latency | similar | similar |

The "remaining" ~410 MB on the native side is the
`tokenizers::Tokenizer` (vocab dictionary + BPE/unigram tables for
500k+ token vocab) and is independent of the embedding matrix.

## Optional int8 quantization sidecar

`from_pretrained_quantized` and `from_hub_quantized` quantize the
matrix to int8 with per-row max-abs scaling at load time
(~4× smaller heap, <1% cosine recovery error). The result is
persisted next to the original as `model.q8.safetensors`; subsequent
loads mmap that sidecar zero-copy. Cosine similarity vs the f32
build is >0.9999 in our tests.

## Usage

Add to `Cargo.toml`:

```toml
[dependencies]
model2vec-native = { git = "https://github.com/taeyun16/model2vec-native" }
```

Load + encode:

```rust
use model2vec_native::StaticModel;

// From a HuggingFace hub repo (cached locally on first use)
let model = StaticModel::from_hub("minishlab/potion-multilingual-128M", None)?;

// Or from a local directory containing tokenizer.json,
// model.safetensors, and config.json
// let model = StaticModel::from_pretrained(&path, None)?;

let v: Vec<f32> = model.encode_single("hello world");
let batch: Vec<Vec<f32>> = model.encode(&["redis sentinel".into(),
                                          "rust async".into()]);

// int8-quantized variant — ~4× smaller heap, sidecar cached on disk
let quant = StaticModel::from_pretrained_quantized(&path, None)?;
```

## API

`StaticModel` mirrors `model2vec_rs::model::StaticModel`:

| Method | Notes |
|---|---|
| `from_pretrained(path, normalize)` | Load f32 mmap from a directory |
| `from_pretrained_quantized(path, normalize)` | + load-time i8 conversion (writes sidecar) |
| `from_hub(repo, normalize)` *(feature `hub`)* | Resolve via hf-hub, then mmap |
| `from_hub_quantized(repo, normalize)` | + i8 conversion |
| `encode_single(&str) -> Vec<f32>` | One vector |
| `encode(&[String]) -> Vec<Vec<f32>>` | Batch |
| `encode_with_args(...)` | Explicit max_length / batch_size |
| `dimension()` | Output vector size |
| `vocab_rows()` | Embedding-matrix row count |

## Compatibility

- Rust 1.85+ (edition 2024)
- Same model files as `model2vec-rs` and the upstream Python
  `model2vec` package — no conversion step needed for f32 models.
- F32 / F16 / I8 source dtypes all supported. F32 stays mmap'd
  zero-copy; F16/I8 widens once into a single Vec<f32>.

## License

Dual-licensed under either:

- MIT license ([LICENSE-MIT](LICENSE-MIT))
- Apache License 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
