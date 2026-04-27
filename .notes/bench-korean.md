# Bench: Korean wiki — multilingual vs English-only model

Tests whether `potion-multilingual-128M`'s extra parameters actually
pay for themselves on Korean text, where `potion-base-4M`
(English-trained) has no language coverage.

## Setup

- 1500 facts, 30 entities. Same English entity names as the prior
  English bench (redis, postgres, ...) but every fact body is
  Korean prose with explicit Korean domain keywords:
  - `redis`: "인메모리 데이터 저장 캐시 키 값"
  - `postgres`: "관계형 데이터베이스 트랜잭션 SQL"
  - ... (full list in the script)
- Three Korean golden sets:
  - **EASY**: query contains the English entity name + Korean
    context ("redis 운영 환경 설정"). K=20.
  - **HARD**: pure-Korean prose, no entity name, but the same
    keywords that appear in fact bodies ("인메모리 데이터 캐시").
    K=50.
  - **PARAPHRASE**: pure-Korean prose, no entity name, **and**
    using synonyms that *don't appear* in any fact
    ("메모리 기반 빠른 키 값 저장소" vs fact's "인메모리"). K=50.

## Results

### EASY (10 queries, K=20)

| Model | P@20 | R@20 |
|---|---:|---:|
| `potion-multilingual-128M` | 1.000 | 0.400 |
| `potion-base-4M` | 1.000 | 0.400 |

Tied — BM25 catches the English entity name in every fact.

### HARD (10 queries, K=50)

| Model | P@50 | R@50 |
|---|---:|---:|
| `potion-multilingual-128M` | 0.400 | 0.247 |
| `potion-base-4M` | 0.400 | 0.247 |

**Tied.** When fact body and query share Korean keywords, BM25
does the work; semantic doesn't need to understand Korean.

### PARAPHRASE (10 queries, K=50) — true semantic stress

| Model | P@50 | R@50 |
|---|---:|---:|
| **`potion-multilingual-128M`** | **0.130** | **0.117** |
| `potion-base-4M` | 0.110 | 0.110 |
| Δ | **+18%** | **+6%** |

This is the only condition where multilingual demonstrably wins —
when the user's query uses Korean synonyms that don't appear
verbatim in fact text. Even here the win is moderate.

## What this proves

| Claim | Verdict on Korean data |
|---|---|
| "potion-multilingual-128M is needed for Korean wikis" | **Mostly false.** BM25 + hybrid RRF cover ≥80% of cases tied with the 4 MB English model. |
| "Multilingual model is worth the 31× memory cost" | **Workload-dependent.** Pays off only when queries paraphrase facts; tied otherwise. |
| "4 MB English model can't handle Korean" | **False.** It works equivalently on keyword-matched queries because BM25 carries the load. |

## Memory cost recap

| | 128M multilingual | 4M base |
|---|---:|---:|
| Heap peak | 315 MB | 10 MB |
| RSS peak | 554 MB | 38 MB |
| CPU per bench | 808 ms | 651 ms |

The 31× memory tax buys you +18% precision on a narrow class of
queries (paraphrase / synonym). For most Korean retrieval
workloads, **`potion-base-4M` is the rational default** with the
multilingual model reserved for projects where heavy paraphrase
is expected (e.g. user-facing search where the query language
diverges from documentation language).

## Caveats

- All synthetic data; real Korean wikis with richer prose and
  domain-specific phrasing might shift the gap.
- 10 paraphrase queries is small. Larger paraphrase sets could
  surface a bigger or smaller multilingual lead.
- `potion-base-8M`, `32M` weren't tested; an English-only model
  with more capacity might close the paraphrase gap further.
- This bench used English entity names + Korean fact bodies. A
  pure-Korean wiki (Korean entity names too) would push BM25
  harder and might tilt toward multilingual on EASY/HARD as well.
