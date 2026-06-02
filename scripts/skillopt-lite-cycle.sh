#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="${WG_SKILLOPT_SOURCE:-$ROOT_DIR/wg-skill/SKILL.md}"
STATE_DIR="${WG_SKILLOPT_STATE_DIR:-$ROOT_DIR/target/skillopt-lite}"
QUEUE_DIR="${WG_SKILLOPT_QUEUE_DIR:-$STATE_DIR/candidates}"
CHECK_SCRIPT="${WG_SKILLOPT_CHECK_SCRIPT:-$ROOT_DIR/scripts/skillopt-lite-check.sh}"
INTERVAL_SECONDS="${WG_SKILLOPT_INTERVAL_SECONDS:-86400}"
MAX_CYCLES="${WG_SKILLOPT_MAX_CYCLES:-1}"
APPLY="${WG_SKILLOPT_APPLY:-0}"
FAIL_ON_REJECT="${WG_SKILLOPT_FAIL_ON_REJECT:-0}"
RUN_SCENARIOS="${WG_SKILLOPT_RUN_SCENARIOS:-0}"

usage() {
    cat <<'EOF'
usage: scripts/skillopt-lite-cycle.sh [--candidate PATH] [--apply] [--interval SECONDS] [--max-cycles N]

Periodic SkillOpt-lite runner for wg memory skill/profile candidates.

Environment:
  WG_SKILLOPT_SOURCE             Source skill/profile to replace on accepted apply.
  WG_SKILLOPT_STATE_DIR          State directory (default: target/skillopt-lite).
  WG_SKILLOPT_QUEUE_DIR          Candidate queue directory (default: $STATE_DIR/candidates).
  WG_SKILLOPT_APPLY=1            Copy accepted candidate over $WG_SKILLOPT_SOURCE.
  WG_SKILLOPT_RUN_SCENARIOS=1    Include Scenario L/M/N in the gate.
  WG_SKILLOPT_MAX_CYCLES=0       Run forever.
  WG_SKILLOPT_FAIL_ON_REJECT=1   Return nonzero when any candidate is rejected.
EOF
}

cli_candidate=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --candidate)
            cli_candidate="${2:?--candidate requires a path}"
            shift 2
            ;;
        --apply)
            APPLY=1
            shift
            ;;
        --interval)
            INTERVAL_SECONDS="${2:?--interval requires seconds}"
            shift 2
            ;;
        --max-cycles)
            MAX_CYCLES="${2:?--max-cycles requires a number}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

mkdir -p "$STATE_DIR"/{accepted,rejected,logs} "$QUEUE_DIR"

json_record() {
    local path="$1"
    shift
    python3 - "$path" "$@" <<'PY'
import json
import sys
import time

path = sys.argv[1]
pairs = sys.argv[2:]
row = {"ts": int(time.time())}
for pair in pairs:
    key, value = pair.split("=", 1)
    row[key] = value
with open(path, "a", encoding="utf-8") as handle:
    handle.write(json.dumps(row, sort_keys=True))
    handle.write("\n")
PY
}

sha256_file() {
    python3 - "$1" <<'PY'
from hashlib import sha256
from pathlib import Path
import sys

print(sha256(Path(sys.argv[1]).read_bytes()).hexdigest())
PY
}

candidate_label() {
    python3 - "$1" <<'PY'
from pathlib import Path
import re
import sys

stem = Path(sys.argv[1]).stem
safe = re.sub(r"[^A-Za-z0-9_.-]+", "-", stem).strip("-")
print(safe or "candidate")
PY
}

run_candidate() {
    local candidate label digest log_path status dest
    candidate="$1"
    label="$(candidate_label "$candidate")"
    digest="$(sha256_file "$candidate")"
    log_path="$STATE_DIR/logs/$(date +%Y%m%dT%H%M%S)-$label-${digest:0:12}.log"

    echo "==> checking candidate $candidate"
    set +e
    WG_SKILLOPT_CANDIDATE="$candidate" \
        WG_SKILLOPT_RUN_SCENARIOS="$RUN_SCENARIOS" \
        "$CHECK_SCRIPT" >"$log_path" 2>&1
    status="$?"
    set -e

    if [[ "$status" == "0" ]]; then
        dest="$STATE_DIR/accepted/$label-${digest:0:12}.md"
        cp "$candidate" "$dest"
        if [[ "$APPLY" == "1" ]]; then
            cp "$SOURCE" "$STATE_DIR/accepted/source-before-$(date +%Y%m%dT%H%M%S).md"
            cp "$candidate" "$SOURCE"
            json_record "$STATE_DIR/runs.jsonl" \
                status=accepted_applied \
                candidate="$candidate" \
                accepted_copy="$dest" \
                source="$SOURCE" \
                sha256="$digest" \
                log="$log_path"
            echo "accepted and applied: $candidate -> $SOURCE"
        else
            json_record "$STATE_DIR/runs.jsonl" \
                status=accepted_dry_run \
                candidate="$candidate" \
                accepted_copy="$dest" \
                source="$SOURCE" \
                sha256="$digest" \
                log="$log_path"
            echo "accepted dry-run: $candidate"
        fi
        return 0
    fi

    dest="$STATE_DIR/rejected/$label-${digest:0:12}.md"
    cp "$candidate" "$dest"
    json_record "$STATE_DIR/rejected_edits.jsonl" \
        status=rejected \
        candidate="$candidate" \
        rejected_copy="$dest" \
        source="$SOURCE" \
        sha256="$digest" \
        log="$log_path" \
        reason="gate_failed_exit_$status"
    echo "rejected: $candidate (log: $log_path)"
    if [[ "$FAIL_ON_REJECT" == "1" ]]; then
        return "$status"
    fi
    return 0
}

collect_candidates() {
    if [[ -n "$cli_candidate" ]]; then
        printf "%s\n" "$cli_candidate"
        return
    fi
    find "$QUEUE_DIR" -maxdepth 1 -type f \( -name '*.md' -o -name 'SKILL.md' \) -print | sort
}

cycle_once() {
    local found candidate
    found=0
    while IFS= read -r candidate; do
        [[ -z "$candidate" ]] && continue
        found=1
        run_candidate "$candidate"
        if [[ -z "$cli_candidate" ]]; then
            case "$candidate" in
                "$QUEUE_DIR"/*)
                    rm -f "$candidate"
                    ;;
            esac
        fi
    done < <(collect_candidates)

    if [[ "$found" == "0" ]]; then
        echo "==> no queued candidate; checking current source profile"
        run_candidate "$SOURCE"
    fi
}

cycle=0
while :; do
    cycle=$((cycle + 1))
    echo "==> skillopt-lite cycle $cycle"
    cycle_once

    if [[ "$MAX_CYCLES" != "0" && "$cycle" -ge "$MAX_CYCLES" ]]; then
        break
    fi
    sleep "$INTERVAL_SECONDS"
done
