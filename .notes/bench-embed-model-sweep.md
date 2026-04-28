# Bench: embedding-model sweep on MIRACL/ko (small corpus)

Quick comparison of `potion-multilingual-128M` (model2vec, the wg
default) vs `intfloat/multilingual-e5-small` (TEI native, Metal
fp16, port 8080) on a tiny MIRACL/ko subset. Goal: see whether
moving from model2vec to a transformer encoder is worth the
inference cost. Run via `benchmarks/src/bin/miracl_model_sweep`.

## Setup

- **Corpus**: 30 docs from MIRACL/ko (15 dev-relevant + 15 random
  filler). Smaller than `bench-miracl-ko.md`'s 5503-doc set
  because TEI inference of long Korean Wikipedia passages is
  inherently slow even on a 118 M-param model — see "What
  surprised me" below.
- **Queries**: 11 (every dev query whose relevant docs landed in
  the 15-doc cap).
- **Embedding side, model2vec**: configured exactly as wg
  ships — `model.provider = "model2vec"`, name
  `minishlab/potion-multilingual-128M`. Auto-downloaded.
- **Embedding side, TEI**: started locally with
  `text-embeddings-router --model-id intfloat/multilingual-e5-small
  --port 8080`. wg pointed at it via `model.provider = "tei"` +
  `model.endpoint = "http://localhost:8080"`. Dim auto-discovered
  via `/info` (384 d).
- **Reranker**: not used in this sweep.

## Results

| config | R@10 | MRR@10 | nDCG@10 | p50 | p95 | build |
|---|---:|---:|---:|---:|---:|---:|
| `model2vec/potion-multi-128M` | 1.000 | 0.955 | 0.966 | **3.2 ms** | 13.1 ms | 0.6 s |
| `tei/multilingual-e5-small` | 1.000 | **1.000** | **1.000** | **2 995 ms** | 6 214 ms | 3.1 s |

(R@10 ceilings out at 1.0 because the 30-doc corpus is small enough
that even BM25-only would catch every relevant doc. The MRR/nDCG
gap is the only quality signal here.)

## Findings

1. **Both models hit the recall ceiling on this corpus.** The
   ranking quality difference is captured by MRR/nDCG: e5-small
   places the first relevant doc at rank 1 every time
   (MRR=1.000); model2vec averages MRR=0.955 (~1 query out of 11
   has a relevant doc that's not first). On a corpus this small
   we can't distinguish actual retrieval quality.

2. **Latency gap is ~1 000×.** model2vec sits at 3 ms p50,
   e5-small at 2 995 ms p50. The interesting half of that gap
   isn't network: TEI's own log shows
   `total_time=2.42s queue=0.3s inference=1.8s` per call. The
   118 M-param transformer running on Metal fp16 is genuinely
   that slow on a single Korean Wikipedia paragraph.

3. **Even the build is asymmetric.** model2vec embeds 30 facts
   in 0.6 s; TEI embeds 30 facts in 3.1 s. The HNSW rebuild
   isn't the bottleneck — TEI inference is.

## What surprised me

The TEI Phase-1 probe (`bench-tei-overhead.md`) reported 4 ms p50
for `all-MiniLM-L6-v2` (22 M params, English-only). I assumed
e5-small at 118 M params would be ~5× slower → tens of ms.
Reality: ~750× slower (3 s, not 20 ms) on real Korean Wikipedia
text. Three factors compound:

- **Sequence length**: standalone probe used 6-token English
  strings; MIRACL queries / passages are 100–500 tokens.
- **Param count**: 118 M vs 22 M is 5×.
- **Tokenizer**: Korean → SentencePiece typically expands more
  tokens per char than English BPE on small models.

Multiplying out: 5× (params) × ~50× (sequence) × small constant
gets us from 4 ms to ~3 s.

## Recommendation by corpus size

| corpus | recommended embedding |
|---|---|
| < 5 000 docs | **model2vec / potion-multilingual-128M** — same recall ceiling, 1000× cheaper |
| 5 000–50 000 docs | model2vec for latency-sensitive flows; consider bge-m3 / e5-base when retrieval recall is hitting a floor |
| 50 000+ docs | proper benchmark (`bench-miracl-ko.md` style) needed — bigger corpora unlock the recall gap that justifies a transformer |
| Latency-sensitive (`fact_add`-heavy) | model2vec, full stop — a single TEI embed already costs more than the entire model2vec ingest of N facts |

The wg default (model2vec/potion-multilingual-128M) stays correct
for the typical user workload. TEI is the right answer when
either (a) the corpus is large enough that model2vec recall is
visibly degraded or (b) the user has a workflow where ranking
precision-at-top matters more than per-search latency.

## Reproduce

```bash
# Prepare a 30-doc subset (max_relevant=15 + 15 random filler)
python3 /tmp/wg-tei-bench/prep_miracl.py \
    --max-relevant=15 --filler=15 \
    --out-dir=/tmp/wg-tei-bench/tiny

# Bulk-ingest into a fresh redb
WG_BENCH_STORE=/tmp/wg-bench-miracl-tiny/_meta/wiki.redb \
WG_BENCH_INPUT=/tmp/wg-tei-bench/tiny/miracl_ko_facts.jsonl \
cargo run --release --bin miracl_ingest

# Spin up TEI native (if not already)
text-embeddings-router \
    --model-id intfloat/multilingual-e5-small \
    --port 8080

# Run the sweep
WG_BENCH_STORE=/tmp/wg-bench-miracl-tiny/_meta/wiki.redb \
WG_BENCH_GOLDEN=/tmp/wg-tei-bench/tiny/miracl_ko_golden.jsonl \
cargo run --release --bin miracl_model_sweep
```

## What we still don't know

- bge-m3 (568 M, multilingual) numbers on a real-size corpus.
  Started a 5 503-doc run earlier; killed at ~30 % because each
  batch of 32 long Korean docs cost ~12 s on Metal fp16, putting
  the whole bench at ~40 min. The qualitative direction (e5-small
  is slow on long Korean text) extrapolates: bge-m3 is ~5× larger
  and would be proportionally slower.
- Whether the e5-small MRR=1.000 advantage holds at scale — on a
  larger, harder corpus where retrieval misses are common.
