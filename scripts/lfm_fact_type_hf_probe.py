#!/usr/bin/env python3
"""Build weak-labelled fact_type probes from public Hugging Face agent traces.

The script uses the Hugging Face Dataset Viewer API, samples public rows, and
emits compact candidate-memory facts. Labels are structural weak labels, not
reviewed truth: tool calls become decision candidates, tool failures become
error candidates, policy constraints become convention candidates, and raw tool
observations remain notes.
"""

from __future__ import annotations

import argparse
import json
import re
import time
import urllib.parse
import urllib.request
from urllib.error import HTTPError
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any


DATASETS = {
    "hermes-kimi": {
        "dataset": "lambda/hermes-agent-reasoning-traces",
        "config": "kimi",
        "split": "train",
    },
    "taubench": {
        "dataset": "sammshen/taubench-sonnet-traces",
        "config": "default",
        "split": "train",
    },
    "swe-smith": {
        "dataset": "SWE-bench/SWE-smith-trajectories",
        "config": "default",
        "split": "tool",
    },
}

LABELS = (
    "preference",
    "decision",
    "lesson",
    "error",
    "convention",
    "pattern",
    "claim",
    "note",
    "question",
)
HIGH_SIGNAL_LABELS = {
    "preference",
    "lesson",
    "error",
    "convention",
    "pattern",
    "claim",
}

EMAIL_RE = re.compile(r"[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}")
LONG_ID_RE = re.compile(r"\b[A-Za-z0-9_#-]{18,}\b")
TOOL_CALL_RE = re.compile(r"<tool_call>(.*?)</tool_call>", re.DOTALL)
TOOL_RESPONSE_RE = re.compile(r"<tool_response>(.*?)</tool_response>", re.DOTALL)


def redact(text: str) -> str:
    text = EMAIL_RE.sub("<email>", text)
    text = LONG_ID_RE.sub("<id>", text)
    return text


def compact(value: Any, limit: int = 420) -> str:
    if isinstance(value, str):
        text = value
    else:
        text = json.dumps(value, ensure_ascii=False, sort_keys=True)
    text = redact(re.sub(r"\s+", " ", text).strip())
    if len(text) > limit:
        return text[: limit - 3].rstrip() + "..."
    return text


def viewer_rows(dataset: str, config: str, split: str, source_rows: int) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    offset = 0
    page_size = min(100, source_rows)
    while len(rows) < source_rows:
        length = min(page_size, source_rows - len(rows))
        query = urllib.parse.urlencode(
            {
                "dataset": dataset,
                "config": config,
                "split": split,
                "offset": offset,
                "length": length,
            }
        )
        url = f"https://datasets-server.huggingface.co/rows?{query}"
        try:
            with urllib.request.urlopen(url, timeout=60) as response:
                payload = json.loads(response.read().decode("utf-8"))
        except HTTPError:
            if page_size <= 1:
                raise
            page_size = max(1, page_size // 2)
            continue
        batch = [item["row"] for item in payload.get("rows") or []]
        if not batch:
            break
        rows.extend(batch)
        offset += len(batch)
        if len(batch) < length:
            break
    return rows


def add(
    out: list[dict[str, Any]],
    *,
    dataset_key: str,
    fact_type: str,
    text: str,
    scenario: str,
    raw_id: str,
) -> None:
    if fact_type not in LABELS:
        return
    text = compact(text)
    if len(text) < 12:
        return
    out.append(
        {
            "id": f"{dataset_key}-{len(out) + 1:05d}",
            "text": text,
            "fact_type": fact_type,
            "scenario": scenario,
            "source": f"huggingface:{dataset_key}",
            "split": "test",
            "label_source": "weak_hf_rule",
            "raw_id": raw_id,
        }
    )


def parse_json_maybe(value: Any) -> Any:
    if not isinstance(value, str):
        return value
    try:
        return json.loads(value)
    except Exception:
        return value


def tool_name_and_args(call: Any) -> tuple[str, Any]:
    call = parse_json_maybe(call)
    if isinstance(call, dict):
        function = call.get("function") or {}
        name = function.get("name") or call.get("name") or ""
        args = function.get("arguments") or call.get("arguments") or {}
        return str(name), parse_json_maybe(args)
    return "", {}


def is_error_text(text: str) -> bool:
    lowered = text.lower().strip()
    return (
        lowered.startswith(("error:", "failed", "failure:", "exception:", "traceback"))
        or "permission denied" in lowered
        or "not found" in lowered[:120]
    )


def is_error_payload(value: Any) -> bool:
    value = parse_json_maybe(value)
    if isinstance(value, dict):
        if value.get("success") is False:
            return True
        if value.get("error") or value.get("error_message"):
            return True
        content = value.get("content")
        if isinstance(content, str):
            return is_error_text(content)
    if isinstance(value, str):
        return is_error_text(value)
    return False


def transform_hermes(dataset_key: str, rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for idx, row in enumerate(rows):
        raw_id = str(row.get("id") or idx)
        task = compact(row.get("task") or "")
        if task:
            add(out, dataset_key=dataset_key, fact_type="question", text=f"Agent task: {task}", scenario="task", raw_id=raw_id)
        category = compact({"category": row.get("category"), "subcategory": row.get("subcategory")})
        add(out, dataset_key=dataset_key, fact_type="pattern", text=f"Hermes trace category: {category}", scenario="category", raw_id=raw_id)
        for msg_idx, msg in enumerate(row.get("conversations") or []):
            role = str(msg.get("from") or msg.get("role") or "")
            value = str(msg.get("value") or msg.get("content") or "")
            msg_id = f"{raw_id}:m{msg_idx}"
            if role == "system":
                for sentence in re.split(r"(?<=[.!?])\s+", value):
                    if any(cue in sentence.lower() for cue in ("do not", "don't", "must", "prefer", "only")):
                        add(
                            out,
                            dataset_key=dataset_key,
                            fact_type="convention",
                            text=f"System instruction: {sentence}",
                            scenario="system_policy",
                            raw_id=msg_id,
                        )
                        break
            for call_match in TOOL_CALL_RE.findall(value):
                name, args = tool_name_and_args(call_match)
                if name:
                    add(
                        out,
                        dataset_key=dataset_key,
                        fact_type="decision",
                        text=f"Hermes tool call: {name} with arguments {compact(args, 220)}.",
                        scenario="tool_call",
                        raw_id=msg_id,
                    )
            for response in TOOL_RESPONSE_RE.findall(value):
                response_text = compact(response, 320)
                if not response_text:
                    continue
                fact_type = "error" if is_error_payload(response) else "note"
                add(
                    out,
                    dataset_key=dataset_key,
                    fact_type=fact_type,
                    text=f"Hermes tool response: {response_text}",
                    scenario="tool_response",
                    raw_id=msg_id,
                )
            if role == "assistant" and any(cue in value.lower() for cue in ("i found", "root cause", "fixed", "confirmed")):
                add(
                    out,
                    dataset_key=dataset_key,
                    fact_type="lesson",
                    text=f"Assistant learned: {value}",
                    scenario="assistant_learning",
                    raw_id=msg_id,
                )
    return out


def transform_taubench(dataset_key: str, rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for idx, row in enumerate(rows):
        raw_id = str(row.get("request_id") or idx)
        method = compact(row.get("method") or "")
        path = compact(row.get("path") or "")
        if method or path:
            add(
                out,
                dataset_key=dataset_key,
                fact_type="pattern",
                text=f"TauBench HTTP trace used {method} {path}.",
                scenario="http_endpoint",
                raw_id=raw_id,
            )
        status_code = row.get("status_code")
        if isinstance(status_code, int) and status_code >= 400:
            add(
                out,
                dataset_key=dataset_key,
                fact_type="error",
                text=f"TauBench HTTP response returned status_code={status_code} for {method} {path}.",
                scenario="http_error",
                raw_id=raw_id,
            )
        metadata = parse_json_maybe(row.get("task_metadata") or {})
        if isinstance(metadata, dict) and metadata:
            add(
                out,
                dataset_key=dataset_key,
                fact_type="claim",
                text=f"TauBench task metadata: {compact(metadata, 260)}.",
                scenario="task_metadata",
                raw_id=raw_id,
            )
        body = parse_json_maybe(row.get("body") or {})
        if not isinstance(body, dict):
            continue
        for msg_idx, msg in enumerate(body.get("messages") or []):
            role = str(msg.get("role") or "")
            content = str(msg.get("content") or "")
            msg_id = f"{raw_id}:m{msg_idx}"
            if role == "system":
                for line in content.splitlines():
                    stripped = line.strip("- ").strip()
                    if any(cue in stripped.lower() for cue in ("must", "should", "only", "do not", "don't")):
                        add(out, dataset_key=dataset_key, fact_type="convention", text=f"TauBench policy: {stripped}", scenario="policy", raw_id=msg_id)
            if role == "user" and content:
                label = "preference" if any(cue in content.lower() for cue in ("prefer", "would like", "want")) else "question"
                add(out, dataset_key=dataset_key, fact_type=label, text=f"TauBench user message: {content}", scenario="user_message", raw_id=msg_id)
            for call in msg.get("tool_calls") or []:
                name, args = tool_name_and_args(call)
                if name:
                    add(out, dataset_key=dataset_key, fact_type="decision", text=f"TauBench tool call: {name} with arguments {compact(args, 220)}.", scenario="tool_call", raw_id=msg_id)
    return out


def transform_swe(dataset_key: str, rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for idx, row in enumerate(rows):
        raw_id = str(row.get("traj_id") or row.get("instance_id") or idx)
        if "resolved" in row:
            add(out, dataset_key=dataset_key, fact_type="claim", text=f"SWE trajectory resolved={row.get('resolved')} for instance {row.get('instance_id')}.", scenario="resolved", raw_id=raw_id)
        messages = parse_json_maybe(row.get("messages") or [])
        if not isinstance(messages, list):
            continue
        for msg_idx, msg in enumerate(messages):
            role = str(msg.get("role") or "")
            content = msg.get("content")
            if isinstance(content, list):
                content = " ".join(str(part.get("text") or part) if isinstance(part, dict) else str(part) for part in content)
            content = str(content or "")
            msg_id = f"{raw_id}:m{msg_idx}"
            if role == "user" and content:
                add(out, dataset_key=dataset_key, fact_type="question", text=f"SWE task request: {content}", scenario="task", raw_id=msg_id)
            action = msg.get("action")
            if action:
                add(out, dataset_key=dataset_key, fact_type="decision", text=f"SWE agent action: {action}", scenario="action", raw_id=msg_id)
            for call in msg.get("tool_calls") or []:
                name, args = tool_name_and_args(call)
                if name:
                    add(out, dataset_key=dataset_key, fact_type="decision", text=f"SWE tool call: {name} with arguments {compact(args, 220)}.", scenario="tool_call", raw_id=msg_id)
            if role == "tool":
                fact_type = "error" if is_error_text(content) else "note"
                add(out, dataset_key=dataset_key, fact_type=fact_type, text=f"SWE tool observation: {content}", scenario="tool_observation", raw_id=msg_id)
            if role == "assistant" and any(cue in content.lower() for cue in ("found the issue", "root cause", "confirmed", "fix works")):
                add(out, dataset_key=dataset_key, fact_type="lesson", text=f"SWE assistant learning: {content}", scenario="assistant_learning", raw_id=msg_id)
    return out


def balanced(rows: list[dict[str, Any]], max_rows: int, max_per_label: int) -> list[dict[str, Any]]:
    seen: set[str] = set()
    by_label: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        key = row["text"].lower()
        if key in seen:
            continue
        seen.add(key)
        by_label[row["fact_type"]].append(row)
    out: list[dict[str, Any]] = []
    counts: Counter[str] = Counter()
    while len(out) < max_rows:
        progressed = False
        for label in LABELS:
            if counts[label] >= max_per_label:
                continue
            bucket = by_label.get(label) or []
            if counts[label] < len(bucket):
                row = dict(bucket[counts[label]])
                row["id"] = f"{row['source'].replace(':', '-').replace('/', '-')}-{len(out) + 1:05d}"
                out.append(row)
                counts[label] += 1
                progressed = True
                if len(out) >= max_rows:
                    break
        if not progressed:
            break
    return out


def write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        for row in rows:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out-dir", required=True, type=Path)
    parser.add_argument("--datasets", default="hermes-kimi,taubench,swe-smith")
    parser.add_argument("--source-rows", type=int, default=100)
    parser.add_argument("--max-rows-per-dataset", type=int, default=100)
    parser.add_argument("--max-per-label", type=int, default=25)
    args = parser.parse_args()

    transformers = {
        "hermes-kimi": transform_hermes,
        "taubench": transform_taubench,
        "swe-smith": transform_swe,
    }
    summary: dict[str, Any] = {}
    combined: list[dict[str, Any]] = []
    for key in [part.strip() for part in args.datasets.split(",") if part.strip()]:
        spec = DATASETS[key]
        started = time.perf_counter()
        source_rows = viewer_rows(spec["dataset"], spec["config"], spec["split"], args.source_rows)
        weak_rows = transformers[key](key, source_rows)
        rows = balanced(weak_rows, args.max_rows_per_dataset, args.max_per_label)
        write_jsonl(args.out_dir / f"{key}_fact_type_probe.jsonl", rows)
        combined.extend(rows)
        summary[key] = {
            "hf_dataset": spec["dataset"],
            "config": spec["config"],
            "split": spec["split"],
            "source_rows": len(source_rows),
            "probe_rows": len(rows),
            "distribution": dict(Counter(row["fact_type"] for row in rows)),
            "fetch_and_build_ms": round((time.perf_counter() - started) * 1000, 2),
        }
    write_jsonl(args.out_dir / "combined_hf_fact_type_probe.jsonl", combined)
    high_signal = [row for row in combined if row["fact_type"] in HIGH_SIGNAL_LABELS]
    write_jsonl(args.out_dir / "high_signal_hf_fact_type_probe.jsonl", high_signal)
    summary["combined"] = {
        "probe_rows": len(combined),
        "distribution": dict(Counter(row["fact_type"] for row in combined)),
    }
    summary["high_signal"] = {
        "probe_rows": len(high_signal),
        "distribution": dict(Counter(row["fact_type"] for row in high_signal)),
    }
    print(json.dumps(summary, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
