#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_PUBLIC_REGISTRY_SMOKE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-public-registry.XXXXXX")}"
SUMMARY_TSV="$BASE/public-registry-smoke.tsv"
MODE="${AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE:-plan}" # plan | verify
VERSION="${AIDEMEMO_PUBLIC_REGISTRY_VERSION:-}"
RUN_RUST="${AIDEMEMO_PUBLIC_REGISTRY_SMOKE_RUST:-1}"
RUN_AGENT_SDK="${AIDEMEMO_PUBLIC_REGISTRY_SMOKE_AGENT_SDK:-1}"
RUN_AGENT_SDK_BINDING="${AIDEMEMO_PUBLIC_REGISTRY_SMOKE_AGENT_SDK_BINDING:-1}"
RUN_HERMES="${AIDEMEMO_PUBLIC_REGISTRY_SMOKE_HERMES:-1}"
RUN_NAPI="${AIDEMEMO_PUBLIC_REGISTRY_SMOKE_NAPI:-1}"
PYTHON_BIN="${AIDEMEMO_PUBLIC_REGISTRY_PYTHON:-python3}"
NPM_BIN="${AIDEMEMO_PUBLIC_REGISTRY_NPM:-npm}"
CARGO_BIN="${AIDEMEMO_PUBLIC_REGISTRY_CARGO:-cargo}"

timer_now() {
    python3 - <<'PY'
import time
print(time.perf_counter())
PY
}

elapsed_since() {
    python3 - "$1" <<'PY'
import sys
import time
start = float(sys.argv[1])
print(f"{time.perf_counter() - start:.2f}")
PY
}

workspace_version() {
    python3 - "$ROOT_DIR" <<'PY'
import sys
import tomllib
from pathlib import Path

root = Path(sys.argv[1])
with (root / "Cargo.toml").open("rb") as handle:
    print(tomllib.load(handle)["workspace"]["package"]["version"])
PY
}

record_row() {
    local status="$1"
    local elapsed="$2"
    local label="$3"
    local detail="$4"
    printf "%s\t%s\t%s\t%s\n" "$status" "$elapsed" "$label" "$detail" >> "$SUMMARY_TSV"
}

record_skip() {
    local label="$1"
    local reason="$2"
    echo "==> skip: $label ($reason)"
    record_row "skip" "-" "$label" "$reason"
}

record_ready() {
    local label="$1"
    local detail="$2"
    echo "==> ready: $label ($detail)"
    record_row "ready" "-" "$label" "$detail"
}

run_timed() {
    local label start status elapsed
    label="$1"
    shift
    echo "==> $label"
    start="$(timer_now)"
    set +e
    "$@"
    status="$?"
    set -e
    elapsed="$(elapsed_since "$start")"
    if [[ "$status" == "0" ]]; then
        record_row "ok" "$elapsed" "$label" ""
    else
        record_row "fail" "$elapsed" "$label" "exit $status"
    fi
    echo "    elapsed: ${elapsed}s"
    return "$status"
}

run_in_dir() {
    local dir="$1"
    shift
    (cd "$dir" && "$@")
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

total = sum(float(elapsed) for status, elapsed, _, _ in rows if status not in {"skip", "ready"})
lines = [
    "## public-registry-smoke",
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

cleanup() {
    print_summary
    if [[ "${AIDEMEMO_PUBLIC_REGISTRY_SMOKE_KEEP_TMP:-0}" == "1" ]]; then
        echo "kept temp dir: $BASE" >&2
    else
        rm -rf "$BASE"
    fi
}

normalize_bool() {
    case "$1" in
        1 | true | TRUE | yes | YES | on | ON) echo "1" ;;
        0 | false | FALSE | no | NO | off | OFF) echo "0" ;;
        *)
            echo "expected boolean-like value, got: $1" >&2
            exit 1
            ;;
    esac
}

if [[ -z "$VERSION" ]]; then
    VERSION="$(workspace_version)"
fi

case "$MODE" in
    plan | verify) ;;
    *)
        echo "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_MODE must be plan or verify (got $MODE)" >&2
        exit 1
        ;;
esac

RUN_RUST="$(normalize_bool "$RUN_RUST")"
RUN_AGENT_SDK="$(normalize_bool "$RUN_AGENT_SDK")"
RUN_AGENT_SDK_BINDING="$(normalize_bool "$RUN_AGENT_SDK_BINDING")"
RUN_HERMES="$(normalize_bool "$RUN_HERMES")"
RUN_NAPI="$(normalize_bool "$RUN_NAPI")"

mkdir -p "$BASE"
: > "$SUMMARY_TSV"
trap cleanup EXIT

echo "public registry smoke"
echo "mode: $MODE"
echo "version: $VERSION"
echo "base: $BASE"
echo

if [[ "$MODE" == "plan" ]]; then
    if [[ "$RUN_RUST" == "1" ]]; then
        record_ready "cargo install aidememo-cli" "$CARGO_BIN install aidememo-cli --version $VERSION --root <tmp>"
    else
        record_skip "cargo install aidememo-cli" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_RUST=0"
    fi
    if [[ "$RUN_AGENT_SDK" == "1" ]]; then
        record_ready "pip install aidememo-agent-sdk" "$PYTHON_BIN -m pip install aidememo-agent-sdk==$VERSION"
    else
        record_skip "pip install aidememo-agent-sdk" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_AGENT_SDK=0"
    fi
    if [[ "$RUN_AGENT_SDK_BINDING" == "1" ]]; then
        record_ready "pip install aidememo-agent-sdk[binding]" "$PYTHON_BIN -m pip install aidememo-agent-sdk[binding]==$VERSION aidememo-python==$VERSION"
    else
        record_skip "pip install aidememo-agent-sdk[binding]" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_AGENT_SDK_BINDING=0"
    fi
    if [[ "$RUN_HERMES" == "1" ]]; then
        record_ready "pip install hermes-aidememo" "$PYTHON_BIN -m pip install hermes-aidememo==$VERSION"
    else
        record_skip "pip install hermes-aidememo" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_HERMES=0"
    fi
    if [[ "$RUN_NAPI" == "1" ]]; then
        record_ready "npm install aidememo-napi" "$NPM_BIN install aidememo-napi@$VERSION"
    else
        record_skip "npm install aidememo-napi" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_NAPI=0"
    fi
    echo
    echo "OK: public registry smoke plan completed"
    exit 0
fi

if [[ "$RUN_RUST" == "1" ]]; then
    CARGO_ROOT="$BASE/cargo-root"
    run_timed "cargo install aidememo-cli" "$CARGO_BIN" install aidememo-cli --version "$VERSION" --root "$CARGO_ROOT"
    run_timed "aidememo public binary help" "$CARGO_ROOT/bin/aidememo" --help
else
    record_skip "cargo install aidememo-cli" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_RUST=0"
fi

if [[ "$RUN_AGENT_SDK" == "1" ]]; then
    AGENT_VENV="$BASE/agent-sdk-venv"
    run_timed "create aidememo-agent-sdk venv" "$PYTHON_BIN" -m venv "$AGENT_VENV"
    run_timed "pip install aidememo-agent-sdk" "$AGENT_VENV/bin/python" -m pip --disable-pip-version-check install "aidememo-agent-sdk==$VERSION"
    run_timed "import aidememo-agent-sdk" "$AGENT_VENV/bin/python" -c "from aidememo_agent import Memory, AideMemoClient, AideMemoMemorySDK, __version__; assert __version__ == '$VERSION'; print('aidememo-agent-sdk', __version__)"
else
    record_skip "pip install aidememo-agent-sdk" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_AGENT_SDK=0"
fi

if [[ "$RUN_AGENT_SDK_BINDING" == "1" ]]; then
    BINDING_VENV="$BASE/agent-sdk-binding-venv"
    run_timed "create aidememo-agent-sdk binding venv" "$PYTHON_BIN" -m venv "$BINDING_VENV"
    run_timed "pip install aidememo-agent-sdk[binding]" "$BINDING_VENV/bin/python" -m pip --disable-pip-version-check install "aidememo-python==$VERSION" "aidememo-agent-sdk[binding]==$VERSION"
    run_timed "import aidememo-python binding" "$BINDING_VENV/bin/python" -c "import aidememo_python; from aidememo_agent import Memory; assert aidememo_python.__version__ == '$VERSION'; print('aidememo-python', aidememo_python.__version__)"
else
    record_skip "pip install aidememo-agent-sdk[binding]" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_AGENT_SDK_BINDING=0"
fi

if [[ "$RUN_HERMES" == "1" ]]; then
    HERMES_VENV="$BASE/hermes-venv"
    run_timed "create hermes-aidememo venv" "$PYTHON_BIN" -m venv "$HERMES_VENV"
    run_timed "pip install hermes-aidememo" "$HERMES_VENV/bin/python" -m pip --disable-pip-version-check install "hermes-aidememo==$VERSION"
    run_timed "import hermes-aidememo" "$HERMES_VENV/bin/python" -c "from hermes_aidememo import AideMemoClient, Memory, register, __version__; assert __version__ == '$VERSION'; print('hermes-aidememo', __version__)"
else
    record_skip "pip install hermes-aidememo" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_HERMES=0"
fi

if [[ "$RUN_NAPI" == "1" ]]; then
    NPM_DIR="$BASE/npm-project"
    mkdir -p "$NPM_DIR"
    run_timed "npm init public smoke project" run_in_dir "$NPM_DIR" "$NPM_BIN" init -y
    run_timed "npm install aidememo-napi" run_in_dir "$NPM_DIR" "$NPM_BIN" install --package-lock=false --no-audit --fund=false "aidememo-napi@$VERSION"
    run_timed "require aidememo-napi" node -e "const am = require('$NPM_DIR/node_modules/aidememo-napi'); if (am.version() !== '$VERSION') { throw new Error(am.version()) } console.log('aidememo-napi', am.version())"
else
    record_skip "npm install aidememo-napi" "AIDEMEMO_PUBLIC_REGISTRY_SMOKE_NAPI=0"
fi

echo
echo "OK: public registry smoke verified"
