#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN_DIR="$ROOT_DIR/plugins/hermes"
SDK_DIR="$ROOT_DIR/packages/aidememo-agent-sdk"
EXPECT_VERSION="${HERMES_AIDEMEMO_EXPECT_VERSION:-}"
BASE="${HERMES_AIDEMEMO_PACK_SMOKE_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/hermes-aidememo-pack-smoke.XXXXXX")}"
SUMMARY_TSV="$BASE/hermes-aidememo-pack-smoke.tsv"
tmp_dir=""

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

record_summary_row() {
    local status elapsed label row_status detail
    status="$1"
    elapsed="$2"
    label="$3"
    if [[ "$status" == "0" ]]; then
        row_status="ok"
        detail=""
    else
        row_status="fail"
        detail="exit $status"
    fi
    printf "%s\t%s\t%s\t%s\n" "$row_status" "$elapsed" "$label" "$detail" >> "$SUMMARY_TSV"
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
    record_summary_row "$status" "$elapsed" "$label"
    echo "    elapsed: ${elapsed}s"
    return "$status"
}

record_fail() {
    local label="$1"
    local reason="$2"
    echo "==> fail: $label ($reason)" >&2
    printf "fail\t0.00\t%s\t%s\n" "$label" "$reason" >> "$SUMMARY_TSV"
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
    "## hermes-aidememo-pack-smoke",
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
    if [[ -n "$tmp_dir" ]]; then
        rm -rf "$tmp_dir"
    fi
}

mkdir -p "$BASE"
: > "$SUMMARY_TSV"
trap cleanup EXIT

version="$(
    python3 - "$PLUGIN_DIR/pyproject.toml" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as f:
    print(tomllib.load(f)["project"]["version"])
PY
)"

if [[ -n "$EXPECT_VERSION" && "$version" != "$EXPECT_VERSION" ]]; then
    record_fail "version expectation" "expected $EXPECT_VERSION but pyproject.toml has $version"
    exit 1
fi

tmp_dir="$(mktemp -d)"
venv_dir="$tmp_dir/venv"
dist_dir="$tmp_dir/dist"
mkdir -p "$dist_dir"

run_timed "create virtualenv" python3 -m venv "$venv_dir"
run_timed "install build backend" "$venv_dir/bin/python" -m pip --disable-pip-version-check install build hatchling
run_timed "build aidememo-agent-sdk wheel" "$venv_dir/bin/python" -m build --wheel --outdir "$dist_dir" "$SDK_DIR"
run_timed "build hermes-aidememo wheel" "$venv_dir/bin/python" -m build --wheel --outdir "$dist_dir" "$PLUGIN_DIR"

sdk_wheel="$(
    find "$dist_dir" -maxdepth 1 -type f -name 'aidememo_agent_sdk-*.whl' | sort | head -n 1
)"
wheel="$(
    find "$dist_dir" -maxdepth 1 -type f -name 'hermes_aidememo-*.whl' | sort | head -n 1
)"
if [[ -z "$sdk_wheel" ]]; then
    record_fail "aidememo-agent-sdk wheel artifact" "missing built aidememo_agent_sdk wheel in $dist_dir"
    exit 1
fi
if [[ -z "$wheel" ]]; then
    record_fail "wheel artifact" "missing built hermes_aidememo wheel in $dist_dir"
    exit 1
fi

run_timed "install built aidememo-agent-sdk wheel" "$venv_dir/bin/python" -m pip --disable-pip-version-check install "$sdk_wheel"
run_timed "install built wheel" "$venv_dir/bin/python" -m pip --disable-pip-version-check install "$wheel"
run_timed "verify installed hermes-aidememo payload" "$venv_dir/bin/python" - "$version" <<'PY'
from pathlib import Path
import importlib.metadata
import sys

from hermes_aidememo import AideMemoClient, AideMemoMemorySDK
from hermes_aidememo import capture_adapter
from hermes_aidememo.client import default_skills_path
from aidememo_agent import Memory
import hermes_aidememo

expected = sys.argv[1]
metadata_version = importlib.metadata.version("hermes-aidememo")
if metadata_version != expected:
    raise SystemExit(f"wheel metadata version {metadata_version} != {expected}")

dist = importlib.metadata.distribution("hermes-aidememo")
entry_points = {
    ep.name: ep.value
    for ep in dist.entry_points
    if ep.group == "hermes.plugins"
}
if entry_points.get("aidememo") != "hermes_aidememo.plugin:register":
    raise SystemExit(f"missing hermes plugin entry point: {entry_points}")

package_dir = Path(hermes_aidememo.__file__).parent
plugin_yaml = package_dir / "plugin.yaml"
skill_md = default_skills_path() / "SKILL.md"
if not plugin_yaml.exists():
    raise SystemExit(f"missing plugin.yaml at {plugin_yaml}")
if not skill_md.exists():
    raise SystemExit(f"missing bundled skill at {skill_md}")
skill_text = skill_md.read_text(encoding="utf-8")
if "Hermes composition recipes" not in skill_text:
    raise SystemExit("bundled skill does not include Hermes composition recipes")
if "aidememo-agent-sdk" not in skill_text:
    raise SystemExit("bundled skill does not mention aidememo-agent-sdk")
if "Memory.open" not in skill_text:
    raise SystemExit("bundled skill does not mention Memory.open")
if "auto_capture.enabled" not in skill_text:
    raise SystemExit("bundled skill does not mention opt-in auto_capture")

sdk = AideMemoMemorySDK.__name__
client = AideMemoClient.__name__
if AideMemoMemorySDK is not Memory:
    raise SystemExit("hermes_aidememo.AideMemoMemorySDK does not re-export aidememo_agent.Memory")
for method in ("open", "search_rows", "remember", "session_canvas", "project_profile"):
    if not hasattr(AideMemoMemorySDK, method):
        raise SystemExit(f"AideMemoMemorySDK missing first-use method: {method}")
cfg = capture_adapter.config_from_plugin({})
if cfg.enabled or cfg.mode != "pending":
    raise SystemExit(f"capture adapter default should be disabled/pending, got {cfg}")
dry = capture_adapter.config_from_plugin({"dry_run": True})
if not dry.enabled or dry.mode != "pending":
    raise SystemExit(f"legacy dry_run should opt into pending capture, got {dry}")
print(f"installed hermes-aidememo {metadata_version}; exports {client}, {sdk}; skill={skill_md}")
PY
