---
title: Evidence
description: A concise scorecard for AideMemo's validated behavior, model placement, and claim boundaries.
---

# Evidence

AideMemo is designed around a local, retrieval-first memory loop. The default
memory path covers capture, typed writes, BM25-first search, and MCP or agent
SDK reads, and does not require an external LLM call. Remote extraction,
embedding, reranking, and reader models remain opt-in.

This page summarizes the results that currently shape product defaults. See the
full [Measurement Ledger](MEASUREMENTS.md) for commands, fixtures, caveats, and
historical runs.

## Validated outcomes

| Surface and result | What it supports |
|---|---|
| **LongMemEval-S retrieval, opt-in BGE plus two-stage rerank, 500 questions**<br />R@10 `0.992`, MRR `0.958` | The semantic path can recover paraphrase-heavy memory when lexical retrieval is not enough. |
| **LongMemEval-S end to end, same retrieval plus MiniMax reader**<br />`74.0%` | Reader-backed evaluation is competitive enough to study, but this is not a default-path or SOTA claim. |
| **BrainBench, BM25 via daemon**<br />P@5 `17.4%`, R@5 `64.1%`; same score and `5.7x` faster than fresh CLI | Keep surface-form-heavy search lexical and keep the store warm. |
| **Shared HTTP MCP, 2 clients x 10 writes**<br />20/20 persisted; p50 `18.4ms`, p95 `41.8ms` | A single local daemon is the preferred concurrent writer path. |
| **Zero-token workflow demo**<br />decision, lesson, and error surfaced in `128ms` | The core workflow can be demonstrated without an agent or model call. |
| **Agent SDK package smoke**<br />wheel install and `Memory`, client, canvas, and profile checks passed in `3.38s` | The code-first integration is independently packageable, not tied to one agent runtime. |

These measurements have different datasets and execution envelopes. Compare
rows within their stated benchmark, not as one aggregate score.

## Model placement

| Failure point and current placement | Evidence boundary |
|---|---|
| **Normal code and docs search**<br />BM25-first `search.auto_hybrid=true`, multilingual model2vec semantic fallback, daemon prewarm | BrainBench stayed quality-equivalent on the lexical daemon path while avoiding fresh-process overhead. |
| **English paraphrase-heavy memory**<br />Opt-in `bge-small-en-v1.5` | LongMemEval-S R@5 improved from `96.2%` to `98.0%`; the roughly `10x` warm query cost is not justified for every workload. |
| **Weak first-stage lexical recall**<br />Guarded MLX LFM embedding experiment | On 180 agent-trace documents and 540 queries, BM25 R@8 was `0.991`, pure LFM dense was `0.887`, and guarded auto reached `0.993` while promoting 2 weak cases. LFM is not the global default embedding replacement. |
| **Good candidates with poor ordering**<br />Warmed LFM ColBERT experiment | A tiny fixture improved hit@1 from `0.57` to `0.86`; candidate recall, document-token cost, and a larger corpus gate must be proven before product placement. |
| **Missing or ambiguous fact type**<br />LFM 1.2B LoRA shadow hint | At confidence `>= 0.98`, the expanded high-signal trace gate accepted 39/155 hints at precision `0.923` with 0 baseline-correct harms. It remains review data, not an automatic write decision. |
| **Privacy-sensitive writes**<br />Opt-in local MLX privacy sidecar plus deterministic secret prefixes | The measured MLX sidecar reduced warm write overhead relative to the CPU model, but memory and latency costs still make explicit enablement the honest default. |

## Claim boundaries

- AideMemo is a memory and retrieval system, not a hosted agent runtime.
- The default memory loop does not call an external LLM. Opt-in extractors,
  TEI endpoints, rerankers, and benchmark readers can.
- Small local models are promoted only where a scenario gate shows a useful
  quality and latency trade-off. A neutral result keeps the cheaper path.
- LongMemEval results calibrate retrieval and reader behavior; AideMemo does not
  lead with a state-of-the-art claim.
- Registry publication status is tracked separately in the
  [Release Checklist](RELEASE.md).

For the system boundary behind these paths, continue with
[Architecture](ARCHITECTURE.md).
