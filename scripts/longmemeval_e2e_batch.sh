#!/usr/bin/env bash
# Batch LongMemEval-S E2E run across the LongMemEval-relevant
# reader tiers, using a single retrieval JSONL as input. Runs each
# reader in the background so the wall-clock is the slowest reader,
# not the sum.
#
# Cost (rough, gpt-4o judge for everything, 500 questions):
#   gpt-4o-mini reader:   ~$1.0    (matches Zep's mini baseline)
#   gpt-4o reader:        ~$5.5    (matches Mem0/Zep/Mastra primary baseline)
#   gpt-4.1 reader:       ~$5.5    (matches OMEGA's reader)
#   gpt-5.4-mini reader:  ~$2.0    (matches Mastra's gpt-5-mini)
#   gpt-5.4 reader:       ~$10     (frontier comparison)
#   ------------------------------
#   total                 ~$24
#
# Usage:
#   set -a; source .env; set +a
#   ./scripts/longmemeval_e2e_batch.sh /tmp/wg_retrievals_500_bge_rerank_wide.jsonl
#
# Each tier writes hypotheses + judgements under
# /tmp/wg_e2e_batch_<reader>/. Fail-soft: if one reader's API key
# tier doesn't allow it, the script logs and moves on.

set -euo pipefail

if [[ -z "${OPENAI_API_KEY:-}" ]]; then
  echo "error: OPENAI_API_KEY not set." >&2
  exit 2
fi

RETRIEVALS="${1:-/tmp/wg_retrievals_500_bge_rerank_wide.jsonl}"
GOLD="${LONGMEMEVAL_DATA:-/tmp/longmemeval/longmemeval_s_cleaned.json}"
JUDGE="${JUDGE:-gpt-4o}"

if [[ ! -f "$RETRIEVALS" ]]; then
  echo "error: retrievals file not found: $RETRIEVALS" >&2
  exit 3
fi
if [[ ! -f "$GOLD" ]]; then
  echo "error: gold dataset not found: $GOLD" >&2
  exit 3
fi

READERS=(
  gpt-4o-mini
  gpt-4o
  gpt-4.1
  gpt-5.4-mini
  gpt-5.4
)

echo "Batch LongMemEval-S E2E"
echo "  retrievals: $RETRIEVALS"
echo "  gold:       $GOLD"
echo "  judge:      $JUDGE"
echo "  readers:    ${READERS[*]}"
echo

declare -a PIDS=()
declare -a LOGS=()
for r in "${READERS[@]}"; do
  outdir="/tmp/wg_e2e_batch_${r}"
  log="/tmp/e2e_batch_${r}.log"
  LOGS+=("$log")
  echo "[launch] reader=$r outdir=$outdir log=$log"
  python3 scripts/longmemeval_e2e.py \
      --retrievals "$RETRIEVALS" \
      --gold "$GOLD" \
      --reader "$r" \
      --judge "$JUDGE" \
      --reader-max-tokens 800 \
      --out "$outdir" \
      > "$log" 2>&1 &
  PIDS+=("$!")
done

echo
echo "Waiting for ${#PIDS[@]} readers ... (tail any /tmp/e2e_batch_*.log for progress)"
fail=0
for i in "${!PIDS[@]}"; do
  pid="${PIDS[$i]}"
  reader="${READERS[$i]}"
  if wait "$pid"; then
    echo "  ✓ $reader done"
  else
    echo "  ✗ $reader failed (see ${LOGS[$i]})"
    fail=$((fail + 1))
  fi
done

echo
echo "Compiled comparison:"
python3 scripts/longmemeval_compile.py --root /tmp || true

if (( fail > 0 )); then
  echo "WARN: $fail reader(s) failed — partial results above." >&2
  exit 4
fi
