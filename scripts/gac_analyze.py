#!/usr/bin/env python3
"""GAC (Geometry-Aware Consolidation) analysis on a wg store.

Stage 1 of the GAC adoption plan from
docs/MEASUREMENTS.md. Pure measurement — no wg
state mutation. Pulls fact contents out of a wg store via
`wg fact list --json`, re-embeds with the same model wg uses
(model2vec / potion-multilingual-128M, L2-normalized to match
the HNSW path), runs hierarchical clustering, and reports for
each retrieval angle θ:

  * cluster count + size distribution
  * tight (d̄ < θ' = 1 - θ) vs spread split
  * fraction of facts that would collapse under GAC at that θ
  * per-cluster d̄ histogram

The numbers tell us what fraction of a real wg store is
GAC-compressible — i.e. how much storage / index-size win is
on the table before we go build Stage 2 (`wg consolidate
--strategy gac`).

Usage:
  python3 scripts/gac_analyze.py \
      --store /tmp/wg-agent-test/wiki.redb \
      --thetas 0.85 0.90 0.95
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

import numpy as np


def fetch_facts(store_path: str, limit: int = 5000) -> list[dict]:
    """`wg fact list --json --limit N` → list of {id, content, ...}."""
    cmd = [
        "/Users/mixlink/dev/wg/target/debug/wg",
        "--store", store_path,
        "fact", "list", "--limit", str(limit), "--json",
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if proc.returncode != 0:
        print(f"error: wg fact list failed: {proc.stderr[:300]}", file=sys.stderr)
        sys.exit(2)
    return json.loads(proc.stdout)


def embed_contents(contents: list[str], model_name: str) -> np.ndarray:
    """Re-embed via the same model wg uses. model2vec ships
    L2-normalized output, matching wg's HNSW insert path."""
    from model2vec import StaticModel
    model = StaticModel.from_pretrained(model_name)
    print(f"  loaded {model_name}: dim={model.dim}", file=sys.stderr)
    embeddings = model.encode(contents, show_progress_bar=False)
    # Ensure L2-normalized (model2vec already does this; double-check)
    norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
    norms[norms == 0] = 1.0
    embeddings = embeddings / norms
    return embeddings.astype(np.float32)


def cluster_at_theta(embeddings: np.ndarray, sim_threshold: float) -> np.ndarray:
    """Single-link hierarchical clustering at cosine-similarity
    threshold. Returns a label array — facts within cosine ≥
    sim_threshold of each other end up in the same cluster.

    For L2-normalized vectors, cosine_similarity = 1 -
    cosine_distance, so we cluster at distance threshold
    (1 - sim_threshold).
    """
    from sklearn.cluster import AgglomerativeClustering
    distance_threshold = 1.0 - sim_threshold
    n = embeddings.shape[0]
    if n < 2:
        return np.zeros(n, dtype=int)
    # Precompute cosine distance (= 1 - dot for L2-normalized).
    sims = embeddings @ embeddings.T
    # Numerical noise can push diagonal slightly off 1.0.
    np.fill_diagonal(sims, 1.0)
    sims = np.clip(sims, -1.0, 1.0)
    distances = 1.0 - sims
    np.fill_diagonal(distances, 0.0)
    distances = np.clip(distances, 0.0, 2.0)

    cluster_model = AgglomerativeClustering(
        n_clusters=None,
        distance_threshold=distance_threshold,
        metric="precomputed",
        linkage="single",
    )
    return cluster_model.fit_predict(distances)


def cluster_mean_distance(embeddings: np.ndarray, members: list[int]) -> float:
    """d̄ = mean within-cluster cosine distance.

    paper's geometric inequality uses this against θ' = 1 - θ
    to decide tight vs spread routing.
    """
    if len(members) < 2:
        return 0.0
    sub = embeddings[members]
    sims = sub @ sub.T
    np.fill_diagonal(sims, np.nan)
    distances = 1.0 - sims
    return float(np.nanmean(distances))


def report_for_theta(
    embeddings: np.ndarray,
    contents: list[str],
    theta: float,
    show_examples: int = 2,
) -> dict:
    """Run GAC analysis at one retrieval half-angle θ. Returns a
    summary dict + prints a per-theta block."""
    theta_prime = 1.0 - theta
    labels = cluster_at_theta(embeddings, theta)
    label_counts = Counter(labels.tolist())

    n_facts = len(labels)
    cluster_sizes = sorted(label_counts.values(), reverse=True)
    n_singleton = sum(1 for s in cluster_sizes if s == 1)
    n_multi = sum(1 for s in cluster_sizes if s >= 2)
    largest = cluster_sizes[0] if cluster_sizes else 0

    tight_count = 0
    spread_count = 0
    tight_facts = 0
    spread_facts = 0
    cluster_summaries: list[tuple[int, int, float]] = []

    for cid, count in label_counts.items():
        if count < 2:
            continue
        members = [i for i, l in enumerate(labels) if l == cid]
        d_bar = cluster_mean_distance(embeddings, members)
        if d_bar < theta_prime:
            tight_count += 1
            tight_facts += count
        else:
            spread_count += 1
            spread_facts += count
        cluster_summaries.append((cid, count, d_bar))

    cluster_summaries.sort(key=lambda x: -x[1])

    # Compression potential: tight cluster non-reps would be
    # collapsed to centroid, spread cluster non-reps go cold.
    # Either way the "kept hot" facts = singletons + 1-rep-per-multi.
    kept_hot = n_singleton + n_multi
    compression = 1.0 - kept_hot / n_facts if n_facts else 0.0

    print()
    print(f"--- θ = {theta:.2f}  (θ' = {theta_prime:.2f}) ---")
    print(f"  facts:        {n_facts}")
    print(f"  clusters:     {len(label_counts)} ({n_singleton} singletons + {n_multi} multi)")
    print(f"  largest:      {largest} facts")
    print(f"  tight × spread: {tight_count} tight clusters ({tight_facts} facts), "
          f"{spread_count} spread ({spread_facts} facts)")
    print(f"  compression:  {n_facts} → {kept_hot} ({compression:.1%} reduction)")
    if cluster_summaries[:show_examples]:
        print(f"  top {show_examples} cluster examples:")
        for cid, count, d_bar in cluster_summaries[:show_examples]:
            tight_marker = "[tight]" if d_bar < theta_prime else "[spread]"
            members = [i for i, l in enumerate(labels) if l == cid][:3]
            sample = contents[members[0]][:80].replace("\n", " ")
            print(f"    {tight_marker} cid={cid} size={count} d̄={d_bar:.3f}")
            print(f"      sample: {sample!r}")

    return {
        "theta": theta,
        "theta_prime": theta_prime,
        "n_facts": n_facts,
        "n_clusters": len(label_counts),
        "n_singletons": n_singleton,
        "n_multi_clusters": n_multi,
        "tight_clusters": tight_count,
        "spread_clusters": spread_count,
        "tight_facts": tight_facts,
        "spread_facts": spread_facts,
        "kept_hot": kept_hot,
        "compression_ratio": compression,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True, help="Path to wiki.redb")
    ap.add_argument("--thetas", type=float, nargs="+", default=[0.85, 0.90, 0.95])
    ap.add_argument("--model", default="minishlab/potion-multilingual-128M",
                    help="model2vec model name (must match wg's embed config)")
    ap.add_argument("--limit", type=int, default=5000,
                    help="Max facts to pull")
    ap.add_argument("--out", type=Path, default=None,
                    help="Optional JSON summary output path")
    args = ap.parse_args()

    print(f"GAC analysis on {args.store}", file=sys.stderr)
    facts = fetch_facts(args.store, args.limit)
    print(f"  pulled {len(facts)} facts", file=sys.stderr)
    if not facts:
        print("error: store is empty", file=sys.stderr)
        return 2

    contents = [f["content"] for f in facts]
    print(f"  embedding via {args.model}…", file=sys.stderr)
    embeddings = embed_contents(contents, args.model)
    print(f"  embeddings: {embeddings.shape}, l2 norm[0]={np.linalg.norm(embeddings[0]):.4f}",
          file=sys.stderr)

    summaries = []
    for theta in args.thetas:
        s = report_for_theta(embeddings, contents, theta)
        summaries.append(s)

    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps({
            "store": args.store,
            "model": args.model,
            "n_facts": len(facts),
            "summaries": summaries,
        }, indent=2))
        print(f"\nwrote: {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
