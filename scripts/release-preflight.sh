#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${WG_RELEASE_PREFLIGHT_PROFILE:-local}" # local | full
BASE="${WG_RELEASE_PREFLIGHT_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/wg-release-preflight.XXXXXX")}"
SUMMARY_TSV="$BASE/release-preflight.tsv"

if [[ "$#" -gt 1 ]]; then
    echo "usage: $0 [semver]" >&2
    echo "example: WG_RELEASE_PREFLIGHT_PROFILE=full $0 0.1.1" >&2
    exit 1
fi

VERSION="${1:-}"

case "$PROFILE" in
    local | full) ;;
    *)
        echo "WG_RELEASE_PREFLIGHT_PROFILE must be local or full (got $PROFILE)" >&2
        exit 1
        ;;
esac

if [[ "$PROFILE" == "full" ]]; then
    RUN_PUBLISH="${WG_RELEASE_PREFLIGHT_PUBLISH:-1}"
    RUN_BINDINGS_OPTIONAL="${WG_RELEASE_PREFLIGHT_BINDINGS_OPTIONAL:-1}"
    REQUIRE_ACTIONLINT="${WG_RELEASE_PREFLIGHT_REQUIRE_ACTIONLINT:-1}"
else
    RUN_PUBLISH="${WG_RELEASE_PREFLIGHT_PUBLISH:-0}"
    RUN_BINDINGS_OPTIONAL="${WG_RELEASE_PREFLIGHT_BINDINGS_OPTIONAL:-0}"
    REQUIRE_ACTIONLINT="${WG_RELEASE_PREFLIGHT_REQUIRE_ACTIONLINT:-0}"
fi

RUN_BINDINGS="${WG_RELEASE_PREFLIGHT_BINDINGS:-1}"
RUN_WORKFLOW="${WG_RELEASE_PREFLIGHT_WORKFLOW:-1}"
RUN_ACTIONLINT="${WG_RELEASE_PREFLIGHT_ACTIONLINT:-1}"

have() {
    command -v "$1" >/dev/null 2>&1
}

record_skip() {
    local label="$1"
    local reason="$2"
    echo "==> skip: $label ($reason)"
    printf "skip\t-\t%s\t%s\n" "$label" "$reason" >> "$SUMMARY_TSV"
}

run() {
    local label start end status elapsed
    label="$1"
    shift
    echo "==> $label"
    start="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    set +e
    "$@"
    status="$?"
    set -e
    end="$(python3 - <<'PY'
import time
print(time.perf_counter())
PY
)"
    elapsed="$(python3 - "$start" "$end" <<'PY'
import sys
start = float(sys.argv[1])
end = float(sys.argv[2])
print(f"{end - start:.2f}")
PY
)"
    if [[ "$status" == "0" ]]; then
        printf "ok\t%s\t%s\t\n" "$elapsed" "$label" >> "$SUMMARY_TSV"
    else
        printf "fail\t%s\t%s\texit %s\n" "$elapsed" "$label" "$status" >> "$SUMMARY_TSV"
    fi
    echo "    elapsed: ${elapsed}s"
    return "$status"
}

print_summary() {
    python3 - "$SUMMARY_TSV" <<'PY'
from pathlib import Path
import os
import sys

rows = []
for line in Path(sys.argv[1]).read_text().splitlines():
    status, elapsed, label, detail = line.split("\t", 3)
    rows.append((status, elapsed, label, detail))

timed = [float(elapsed) for status, elapsed, _, _ in rows if status != "skip"]
total = sum(timed)
lines = [
    "## release-preflight",
    "",
    "| Status | Step | Seconds | Detail |",
    "|---|---|---:|---|",
]
for status, elapsed, label, detail in rows:
    lines.append(f"| {status} | `{label}` | {elapsed} | {detail} |")
lines.append(f"| total | | {total:.2f} | |")

text = "\n".join(lines)
print(text)

summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
if summary_path:
    with open(summary_path, "a", encoding="utf-8") as handle:
        handle.write(text)
        handle.write("\n")
PY
}

cd "$ROOT_DIR"
mkdir -p "$BASE"
: > "$SUMMARY_TSV"
trap print_summary EXIT

echo "release preflight"
echo "profile: $PROFILE"
echo "base: $BASE"
echo

if [[ -n "$VERSION" ]]; then
    run "release version gate" "$ROOT_DIR/scripts/wg-release-version.sh" "$VERSION"
else
    run "release version gate" "$ROOT_DIR/scripts/wg-release-version.sh"
fi

if [[ "$RUN_ACTIONLINT" == "1" ]]; then
    if have actionlint; then
        run "workflow syntax lint" actionlint .github/workflows/*.yml
    elif [[ "$REQUIRE_ACTIONLINT" == "1" ]]; then
        echo "actionlint is required for full release preflight" >&2
        exit 1
    else
        record_skip "workflow syntax lint" "actionlint not installed"
    fi
else
    record_skip "workflow syntax lint" "WG_RELEASE_PREFLIGHT_ACTIONLINT=0"
fi

if [[ "$RUN_BINDINGS" == "1" ]]; then
    run "binding release smoke" env \
        WG_BINDINGS_SMOKE_OPTIONAL="$RUN_BINDINGS_OPTIONAL" \
        "$ROOT_DIR/scripts/bindings-release-smoke.sh"
else
    record_skip "binding release smoke" "WG_RELEASE_PREFLIGHT_BINDINGS=0"
fi

if [[ "$RUN_WORKFLOW" == "1" ]]; then
    run "workflow release smoke" "$ROOT_DIR/scripts/workflow-release-smoke.sh"
else
    record_skip "workflow release smoke" "WG_RELEASE_PREFLIGHT_WORKFLOW=0"
fi

if [[ "$RUN_PUBLISH" == "1" ]]; then
    run "wg-python publish dry-run" "$ROOT_DIR/scripts/wg-python-publish-dry-run.sh"
    run "wg-napi publish dry-run" "$ROOT_DIR/scripts/wg-napi-publish-dry-run.sh"
else
    record_skip "wg-python publish dry-run" "set WG_RELEASE_PREFLIGHT_PROFILE=full"
    record_skip "wg-napi publish dry-run" "set WG_RELEASE_PREFLIGHT_PROFILE=full"
fi

echo
echo "OK: release preflight completed"
