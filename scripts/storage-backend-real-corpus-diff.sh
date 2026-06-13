#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${AIDEMEMO_STORAGE_CORPUS_BASE:-$(mktemp -d "${TMPDIR:-/tmp}/aidememo-storage-corpus.XXXXXX")}"
BIN="$ROOT_DIR/target/debug/aidememo"

cleanup() {
    if [[ "${AIDEMEMO_STORAGE_CORPUS_KEEP_TMP:-0}" != "1" ]]; then
        rm -rf "$BASE"
    else
        echo "kept temp dir: $BASE" >&2
    fi
}
trap cleanup EXIT

cd "$ROOT_DIR"
cargo build -p aidememo-cli --features sqlite

REDB_HOME="$BASE/home-redb"
SQLITE_HOME="$BASE/home-sqlite"
CORPUS_DIR="$BASE/corpus"
REDB_STORE="$BASE/real.redb"
SQLITE_STORE="$BASE/real.sqlite"
REDB_EXPORT="$BASE/redb.jsonl"
SQLITE_EXPORT="$BASE/sqlite.jsonl"
mkdir -p "$REDB_HOME" "$SQLITE_HOME" "$CORPUS_DIR"

python3 - "$ROOT_DIR" "$CORPUS_DIR" <<'PY'
from pathlib import Path
import shutil
import sys

root = Path(sys.argv[1])
dest = Path(sys.argv[2])
files = [
    "README.md",
    "AGENTS.md",
    "docs/INTRODUCTION.md",
    "docs/QUICKSTART.md",
    "docs/FEATURES.md",
    "docs/CLI.md",
    "docs/MCP.md",
    "docs/OPERATIONS.md",
    "docs/SDK.md",
    "docs/SDK_POSITIONING.md",
    "docs/MEASUREMENTS.md",
    "docs/INSTALLATION.md",
    "docs/RELEASE.md",
]
for rel in files:
    src = root / rel
    if not src.exists():
        raise SystemExit(f"missing corpus source: {rel}")
    out = dest / rel
    out.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, out)
PY

HOME="$REDB_HOME" "$BIN" --store "$REDB_STORE" ingest "$CORPUS_DIR" >/dev/null
HOME="$SQLITE_HOME" "$BIN" config set store.backend sqlite >/dev/null
HOME="$SQLITE_HOME" "$BIN" --store "$SQLITE_STORE" ingest "$CORPUS_DIR" >/dev/null

redb_stats="$(HOME="$REDB_HOME" "$BIN" --store "$REDB_STORE" --json stats)"
sqlite_stats="$(HOME="$SQLITE_HOME" "$BIN" --store "$SQLITE_STORE" --json stats)"
python3 - "$redb_stats" "$sqlite_stats" <<'PY'
from collections import Counter
import json
import sys

redb = json.loads(sys.argv[1])
sqlite = json.loads(sys.argv[2])
for key in ("entity_count", "fact_count", "relation_count"):
    if redb[key] != sqlite[key]:
        raise SystemExit(f"{key} mismatch: redb={redb[key]} sqlite={sqlite[key]}")
if sqlite["entity_count"] < 10 or sqlite["fact_count"] < 40:
    raise SystemExit(f"corpus too small for parity gate: {sqlite}")
PY

HOME="$REDB_HOME" "$BIN" --store "$REDB_STORE" export --output "$REDB_EXPORT" >/dev/null
HOME="$SQLITE_HOME" "$BIN" --store "$SQLITE_STORE" export --output "$SQLITE_EXPORT" >/dev/null

python3 - "$REDB_EXPORT" "$SQLITE_EXPORT" <<'PY'
from collections import Counter
from pathlib import Path
import json
import sys


def stable(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def load(path):
    entities = {}
    records = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line:
            continue
        obj = json.loads(line)
        kind = obj.get("type")
        if not kind:
            continue
        data = obj["data"]
        records.append((kind, data))
        if kind == "entity":
            entities[data["id"]] = data["name"]
    entity_rows = []
    fact_rows = []
    relation_rows = []
    for kind, data in records:
        if kind == "entity":
            entity_rows.append(
                (
                    data["name"],
                    stable(data.get("entity_type")),
                    tuple(sorted(data.get("aliases") or [])),
                    tuple(sorted(data.get("tags") or [])),
                    data.get("source_page"),
                )
            )
        elif kind == "fact":
            fact_rows.append(
                (
                    data["content"],
                    stable(data.get("fact_type")),
                    tuple(sorted(entities.get(eid, f"<missing:{eid}>") for eid in data.get("entity_ids") or [])),
                    tuple(sorted(data.get("tags") or [])),
                    data.get("source"),
                    data.get("source_id"),
                    data.get("observed_at"),
                )
            )
        elif kind == "relation":
            relation_rows.append(
                (
                    entities.get(data["source_id"], f"<missing:{data['source_id']}>"),
                    stable(data.get("relation_type")),
                    entities.get(data["target_id"], f"<missing:{data['target_id']}>"),
                    round(float(data.get("weight", 1.0)), 6),
                    tuple(sorted(data.get("evidence") or [])),
                )
            )
    return {
        "entities": Counter(entity_rows),
        "facts": Counter(fact_rows),
        "relations": Counter(relation_rows),
    }


left = load(sys.argv[1])
right = load(sys.argv[2])
for bucket in ("entities", "facts", "relations"):
    if left[bucket] != right[bucket]:
        missing = left[bucket] - right[bucket]
        extra = right[bucket] - left[bucket]
        raise SystemExit(
            f"{bucket} mismatch\n"
            f"missing from sqlite: {missing.most_common(5)}\n"
            f"extra in sqlite: {extra.most_common(5)}"
        )
PY

python3 - "$BIN" "$REDB_HOME" "$REDB_STORE" "$SQLITE_HOME" "$SQLITE_STORE" <<'PY'
from collections import Counter
import json
import os
import subprocess
import sys

bin_path, redb_home, redb_store, sqlite_home, sqlite_store = sys.argv[1:]
queries = [
    "MCP server",
    "SQLite backend",
    "storage backend",
    "AideMemo SDK",
    "daemon",
    "archive",
    "search",
    "rerank",
]


def run_search(home, store, query):
    env = os.environ.copy()
    env["HOME"] = home
    proc = subprocess.run(
        [bin_path, "--store", store, "--json", "search", query, "--limit", "8"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
        env=env,
    )
    rows = json.loads(proc.stdout)
    if not rows:
        raise SystemExit(f"query returned no hits: {query!r}")
    return [
        (
            row["content"],
            row["fact_type"],
            tuple(sorted(row.get("entity_names") or [])),
            row.get("source"),
        )
        for row in rows
    ]


for query in queries:
    redb = run_search(redb_home, redb_store, query)
    sqlite = run_search(sqlite_home, sqlite_store, query)
    if redb[0] != sqlite[0] or Counter(redb) != Counter(sqlite):
        raise SystemExit(
            f"search mismatch for {query!r}\n"
            f"redb={redb[:5]}\n"
            f"sqlite={sqlite[:5]}"
        )
PY

echo "storage backend real-corpus diff ok: docs corpus ingest/export/search parity"
