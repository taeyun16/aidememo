#!/usr/bin/env bash
# Verify OPENAI_API_KEY works and surface which models we can call.
#
# Usage:
#   ./scripts/openai_check.sh                  # uses $OPENAI_API_KEY
#   set -a; source .env; set +a; ./scripts/openai_check.sh
#
# What it does:
#   1. Confirms the key is set.
#   2. Pings GET /v1/models — auth + quota check (no spend).
#   3. Filters the model list to the ones LongMemEval cares about
#      (gpt-4o-mini / gpt-4.1 / gpt-5.4* / gpt-5.5*) so you can see
#      which tiers are actually accessible from your key.
#   4. Optional: pass `--ping` to fire one tiny chat call against
#      gpt-4o-mini (~$0.0001) to confirm completions work end-to-end.

set -euo pipefail

if [[ -z "${OPENAI_API_KEY:-}" ]]; then
  echo "error: OPENAI_API_KEY not set." >&2
  echo "  export it, or run: set -a; source .env; set +a" >&2
  exit 2
fi

# Mask the key for any echoed lines.
key_preview="${OPENAI_API_KEY:0:7}…${OPENAI_API_KEY: -4}"
echo "[1/3] Key present: ${key_preview}"

echo "[2/3] GET /v1/models …"
http_status=$(curl -sS -o /tmp/openai_models.json -w '%{http_code}' \
    -H "Authorization: Bearer ${OPENAI_API_KEY}" \
    "https://api.openai.com/v1/models")

if [[ "$http_status" != "200" ]]; then
  echo "  HTTP ${http_status} — auth failed?"
  echo "  body:"
  cat /tmp/openai_models.json | head -c 500
  echo
  exit 3
fi

# Count + filter to LongMemEval-relevant tiers.
echo "  HTTP 200 — auth OK."
all_count=$(python3 -c "import json; print(len(json.load(open('/tmp/openai_models.json'))['data']))")
echo "  total models accessible: ${all_count}"
echo
echo "  models matching LongMemEval recommendation:"
python3 -c "
import json, re
data = json.load(open('/tmp/openai_models.json'))['data']
ids = sorted(m['id'] for m in data)
pat = re.compile(r'^(gpt-4o(-mini)?|gpt-4\.1.*|gpt-5\.[0-9]+(-mini|-nano)?|o[0-9].*)$')
matched = [m for m in ids if pat.match(m)]
if matched:
    for m in matched:
        print(f'    {m}')
else:
    print('    (none — your key may be on a tier that does not include these)')"
echo

if [[ "${1:-}" == "--ping" ]]; then
  # Ping the LongMemEval recommendation set so the operator knows
  # which tiers are actually callable, not just listed. Each ping
  # is ~$0.00005-0.001 depending on model — total under $0.01.
  PING_MODELS=(
    gpt-4o-mini
    gpt-4o
    gpt-4.1
    gpt-4.1-mini
    gpt-5.4-mini
    gpt-5.4
  )
  echo "[3/3] Pinging ${#PING_MODELS[@]} candidate models (~\$0.01 total) …"
  printf '  %-20s  %-6s  %s\n' MODEL HTTP NOTE
  printf '  %-20s  %-6s  %s\n' --------------------- ------ ---------------
  any_ok=0
  for model in "${PING_MODELS[@]}"; do
    # gpt-5.x and o-series rejected `max_tokens` — they want
    # `max_completion_tokens` (the post-reasoning-models naming).
    # Older 4o/4.1 still accept `max_tokens`.
    case "$model" in
      gpt-5*|o1*|o3*|o4*)
        token_field='"max_completion_tokens":16' ;;
      *)
        token_field='"max_tokens":5' ;;
    esac
    body=$(printf '{"model":"%s","messages":[{"role":"user","content":"Reply with: OK"}],%s}' "$model" "$token_field")
    status=$(curl -sS -o /tmp/openai_ping.json -w '%{http_code}' \
        -H "Authorization: Bearer ${OPENAI_API_KEY}" \
        -H "Content-Type: application/json" \
        -d "$body" \
        "https://api.openai.com/v1/chat/completions" || echo "ERR")
    if [[ "$status" == "200" ]]; then
      reply=$(python3 -c "import json; print(json.load(open('/tmp/openai_ping.json'))['choices'][0]['message']['content'].strip()[:30])")
      printf '  %-20s  %-6s  %s\n' "$model" "$status" "✓ '$reply'"
      any_ok=1
    elif [[ "$status" == "404" ]]; then
      printf '  %-20s  %-6s  %s\n' "$model" "$status" "model not found / not enabled"
    elif [[ "$status" == "403" ]]; then
      printf '  %-20s  %-6s  %s\n' "$model" "$status" "no access (tier-locked?)"
    elif [[ "$status" == "429" ]]; then
      printf '  %-20s  %-6s  %s\n' "$model" "$status" "rate-limit / quota"
    else
      err=$(python3 -c "import json; r=json.load(open('/tmp/openai_ping.json')); print(r.get('error',{}).get('message','')[:60])" 2>/dev/null || echo '')
      printf '  %-20s  %-6s  %s\n' "$model" "$status" "$err"
    fi
  done
  echo
  if [[ "$any_ok" == "1" ]]; then
    echo "  ✓ completion path verified for at least one model."
  else
    echo "  ✗ no model was callable — check billing / tier."
    exit 4
  fi
else
  echo "[3/3] skipped completion ping. Run with --ping to verify which models actually call (cost <\$0.01)."
fi
