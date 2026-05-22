#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_NPM="${WG_BINDINGS_SMOKE_NPM:-1}"
RUN_OPTIONAL="${WG_BINDINGS_SMOKE_OPTIONAL:-0}"
BASE="${WG_BINDINGS_SMOKE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/wg-bindings-smoke.XXXXXX")}"
SUMMARY_TSV="$BASE/bindings-release-smoke.tsv"

run() {
    local label start end status elapsed
    label="$*"
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

have() {
    command -v "$1" >/dev/null 2>&1
}

status_line() {
    printf '%-14s %-8s %s\n' "$1" "$2" "$3"
}

record_status() {
    status_line "$1" "$2" "$3"
    printf "%s\t-\t%s\t%s\n" "$2" "$1" "$3" >> "$SUMMARY_TSV"
}

print_summary() {
    if [[ ! -s "$SUMMARY_TSV" ]]; then
        return
    fi

    python3 - "$SUMMARY_TSV" <<'PY'
from pathlib import Path
import os
import sys

rows = []
for line in Path(sys.argv[1]).read_text().splitlines():
    status, elapsed, label, detail = line.split("\t", 3)
    rows.append((status, elapsed, label, detail))

total = sum(float(elapsed) for _, elapsed, _, _ in rows if elapsed != "-")
lines = [
    "## bindings-release-smoke",
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

echo "binding release smoke"
echo
status_line "binding" "status" "detail"
status_line "-------" "------" "------"

run cargo check -p wg-python -p wg-napi -p wg-nif -p wg-ffi

if [[ "$RUN_NPM" == "1" ]]; then
    run scripts/wg-napi-version.sh
    run scripts/wg-napi-pack-smoke.sh
    record_status "wg-napi" "ok" "version gate + root/platform pack/install smoke"
else
    record_status "wg-napi" "skip" "set WG_BINDINGS_SMOKE_NPM=1 to run npm pack/install smoke"
fi

if have maturin; then
    if [[ "$RUN_OPTIONAL" == "1" ]]; then
        run scripts/wg-python-pack-smoke.sh
        record_status "wg-python" "ok" "version gate + wheel install smoke"
    else
        run scripts/wg-python-version.sh
        record_status "wg-python" "ready" "maturin found; set WG_BINDINGS_SMOKE_OPTIONAL=1 to run wheel install smoke"
    fi
else
    record_status "wg-python" "todo" "install maturin, then run scripts/wg-python-pack-smoke.sh"
fi

if have mix; then
    if [[ "$RUN_OPTIONAL" == "1" ]]; then
        if [[ ! -d crates/wg-nif/deps/jason ]]; then
            run bash -lc 'cd crates/wg-nif && mix deps.get'
        fi
        run bash -lc 'cd crates/wg-nif && mix compile.cargo --force && mix test'
        record_status "wg-nif" "ok" "mix compile.cargo --force && mix test"
    else
        record_status "wg-nif" "ready" "mix found; set WG_BINDINGS_SMOKE_OPTIONAL=1 to run mix test"
    fi
else
    record_status "wg-nif" "todo" "install Elixir/Mix, then run: cd crates/wg-nif && mix deps.get && mix compile.cargo --force && mix test"
fi

if have cc; then
    if [[ "$RUN_OPTIONAL" == "1" ]]; then
        run cargo build -p wg-ffi
        run cc crates/wg-ffi/example/smoke.c -I crates/wg-ffi/include -L target/debug -lwg_ffi -o target/wg-ffi-smoke
        case "$(uname -s)" in
            Darwin)
                run env DYLD_LIBRARY_PATH="$ROOT_DIR/target/debug:${DYLD_LIBRARY_PATH:-}" target/wg-ffi-smoke
                ;;
            Linux)
                run env LD_LIBRARY_PATH="$ROOT_DIR/target/debug:${LD_LIBRARY_PATH:-}" target/wg-ffi-smoke
                ;;
            *)
                run target/wg-ffi-smoke
                ;;
        esac
        record_status "wg-ffi" "ok" "C smoke linked against target/debug/libwg_ffi"
    else
        record_status "wg-ffi" "ready" "cc found; set WG_BINDINGS_SMOKE_OPTIONAL=1 to run C smoke"
    fi
else
    record_status "wg-ffi" "todo" "install a C compiler, then run the README smoke"
fi

echo
echo "OK: binding release smoke completed"
