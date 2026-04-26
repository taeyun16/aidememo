//! Smoke test: load the cached potion-multilingual-128M model and
//! verify a deterministic encode produces a 256-dim vector.
//!
//! Skipped if the HF cache snapshot directory isn't present locally.
//! Designed to never block CI: this is a "did mmap-loading work on a
//! real model?" sanity check, not a relevance test.

use std::path::PathBuf;

fn cache_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let snap_dir = PathBuf::from(home)
        .join(".cache/huggingface/hub")
        .join("models--minishlab--potion-multilingual-128M")
        .join("snapshots");
    let mut entries = std::fs::read_dir(&snap_dir).ok()?;
    let first = entries.next()?.ok()?;
    let p = first.path();
    if p.join("model.safetensors").exists() {
        Some(p)
    } else {
        None
    }
}

#[test]
fn loads_real_model_and_encodes() {
    let dir = match cache_path() {
        Some(p) => p,
        None => {
            eprintln!("skipping: model not present in HF cache");
            return;
        }
    };

    let model =
        model2vec_native::StaticModel::from_pretrained(&dir, None).expect("load model from cache");
    assert_eq!(model.dimension(), 256, "potion model dim is 256");
    assert!(model.vocab_rows() > 100_000, "vocab should be substantial");

    let v = model.encode_single("hello world");
    assert_eq!(v.len(), 256, "vector dim matches model dim");
    let nonzero = v.iter().filter(|&&x| x != 0.0).count();
    assert!(
        nonzero > 10,
        "encoding produced near-zero vector: {}",
        nonzero
    );

    // Deterministic re-encoding.
    let v2 = model.encode_single("hello world");
    assert_eq!(v, v2);
}

#[test]
fn quantized_load_close_to_f32() {
    // Cosine between f32 and i8-quantized encodings of the same text
    // should be very high (>0.99). This catches gross quantization
    // bugs without pretending to be a relevance test.
    let dir = match cache_path() {
        Some(p) => p,
        None => return,
    };

    let f32_model = model2vec_native::StaticModel::from_pretrained(&dir, None).expect("load f32");
    let i8_model =
        model2vec_native::StaticModel::from_pretrained_quantized(&dir, None).expect("load i8");

    let text = "Redis Sentinel provides high availability";
    let a = f32_model.encode_single(text);
    let b = i8_model.encode_single(text);

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    let cos = dot / (na * nb + 1e-12);
    assert!(
        cos > 0.99,
        "cosine(f32, i8) = {} — quantization loss too large",
        cos
    );
}

#[test]
fn batch_encode_matches_single() {
    let dir = match cache_path() {
        Some(p) => p,
        None => return,
    };

    let model =
        model2vec_native::StaticModel::from_pretrained(&dir, None).expect("load model from cache");

    let texts = vec!["redis sentinel".to_string(), "rust async".to_string()];
    let batch = model.encode(&texts);
    let one = model.encode_single(&texts[0]);
    let two = model.encode_single(&texts[1]);

    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0], one);
    assert_eq!(batch[1], two);
}
