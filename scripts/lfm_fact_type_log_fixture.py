#!/usr/bin/env python3
"""Build weak-labelled fact_type probes from local agent behavior logs.

The output is intentionally a probe, not a reviewed training set. It extracts
only structurally clear events from AgentStep traces and Hermes session logs:
target actions, cache hits, evidence claims, user questions, and explicit
session instructions. This lets us test whether the LFM fact_type sidecar is
useful on real agent-behavior text without sending the logs to an external LLM.
"""

from __future__ import annotations

import argparse
import json
import os
import re
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any


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


def compact(value: Any, limit: int = 240) -> str:
    text = json.dumps(value, ensure_ascii=False, sort_keys=True) if not isinstance(value, str) else value
    text = re.sub(r"\s+", " ", text).strip()
    if len(text) > limit:
        return text[: limit - 3].rstrip() + "..."
    return text


def add(
    rows: list[dict[str, Any]],
    *,
    dataset: str,
    source: str,
    fact_type: str,
    text: str,
    scenario: str,
    raw_id: str,
) -> None:
    if fact_type not in LABELS:
        return
    text = compact(text, 420)
    if len(text) < 12:
        return
    rows.append(
        {
            "id": f"{dataset}-{len(rows) + 1:04d}",
            "text": text,
            "fact_type": fact_type,
            "scenario": scenario,
            "source": source,
            "split": "test",
            "label_source": "weak_log_rule",
            "raw_id": raw_id,
        }
    )


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def agentstep_rows(root: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    traces_dir = root / "fixtures" / "traces"
    for path in sorted(traces_dir.glob("*.json")):
        try:
            trace = read_json(path)
        except Exception:
            continue
        run_id = str(trace.get("run_id") or path.stem)
        user_task = compact(trace.get("user_task") or "")
        if user_task:
            add(
                rows,
                dataset="agentstep",
                source=str(path),
                fact_type="note",
                text=f"Observed agent task: {user_task}",
                scenario="trace_task",
                raw_id=run_id,
            )
        seen_patterns: set[tuple[str, str]] = set()
        for step in trace.get("steps") or []:
            step_id = str(step.get("step_id") or "")
            step_type = str(step.get("type") or "step")
            status = str(step.get("status") or "")
            output = compact(step.get("output_summary") or step.get("input_summary") or "")
            if step.get("model_call") and output:
                add(
                    rows,
                    dataset="agentstep",
                    source=str(path),
                    fact_type="decision",
                    text=f"Route output for {step_type}: {output}",
                    scenario="model_route",
                    raw_id=f"{run_id}:{step_id}",
                )
            tool_call = step.get("tool_call") or {}
            tool_name = str(tool_call.get("tool_name") or "")
            if tool_name:
                key = (tool_name, step_type)
                if key not in seen_patterns:
                    seen_patterns.add(key)
                    add(
                        rows,
                        dataset="agentstep",
                        source=str(path),
                        fact_type="pattern",
                        text=f"Tool pattern: {tool_name} is used for {step_type} steps in run {run_id}.",
                        scenario="tool_pattern",
                        raw_id=f"{run_id}:{step_id}:pattern",
                    )
                if tool_call.get("cache_hit") or status == "skipped":
                    add(
                        rows,
                        dataset="agentstep",
                        source=str(path),
                        fact_type="lesson",
                        text=(
                            f"Cache-hit behavior: {tool_name} reused prior output for "
                            f"{compact(tool_call.get('arguments') or {})}; result {output}"
                        ),
                        scenario="cache_reuse",
                        raw_id=f"{run_id}:{step_id}",
                    )
            if status and status not in {"success", "skipped", "ok"}:
                add(
                    rows,
                    dataset="agentstep",
                    source=str(path),
                    fact_type="error",
                    text=f"Step failure: {step_type} ended with status {status}; output {output}",
                    scenario="step_failure",
                    raw_id=f"{run_id}:{step_id}",
                )
            for idx, evidence in enumerate(step.get("evidence") or []):
                claim = compact(evidence.get("claim") or "")
                if claim:
                    add(
                        rows,
                        dataset="agentstep",
                        source=str(path),
                        fact_type="claim",
                        text=f"Evidence claim: {claim}",
                        scenario="trace_evidence",
                        raw_id=f"{run_id}:{step_id}:ev{idx}",
                    )

    router_path = root / "datasets" / "router_trace_v0.jsonl"
    if router_path.exists():
        with router_path.open(encoding="utf-8") as f:
            for line in f:
                if not line.strip():
                    continue
                row = json.loads(line)
                target = row.get("target") or {}
                meta = row.get("metadata") or {}
                inp = row.get("input") or {}
                action = str(target.get("action_id") or "")
                tool_name = str(target.get("tool_name") or "")
                req = compact(inp.get("current_request") or "")
                if action == "reuse_cached_result":
                    add(
                        rows,
                        dataset="agentstep",
                        source=str(router_path),
                        fact_type="lesson",
                        text=f"Router reused cached result for {tool_name}; request {req}.",
                        scenario="router_cache",
                        raw_id=str(row.get("example_id")),
                    )
                elif action:
                    add(
                        rows,
                        dataset="agentstep",
                        source=str(router_path),
                        fact_type="decision",
                        text=(
                            f"Router target action {action} for request {req}; "
                            f"tool {tool_name}; confidence {target.get('confidence')}."
                        ),
                        scenario="router_target",
                        raw_id=str(row.get("example_id")),
                    )
                if meta.get("error") or str(meta.get("observed_status") or "") == "failed":
                    add(
                        rows,
                        dataset="agentstep",
                        source=str(router_path),
                        fact_type="error",
                        text=f"Router observed failure for {tool_name}: {compact(meta.get('error') or meta)}",
                        scenario="router_failure",
                        raw_id=str(row.get("example_id")),
                    )
    return rows


QUESTION_RE = re.compile(r"Question:\s*(.+)", re.IGNORECASE | re.DOTALL)


def hermes_rows(session_dir: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for path in sorted(session_dir.glob("session_*.json")):
        try:
            session = read_json(path)
        except Exception:
            continue
        session_id = str(session.get("session_id") or path.stem)
        model = compact(session.get("model") or "")
        platform = compact(session.get("platform") or "")
        if model or platform:
            add(
                rows,
                dataset="hermes",
                source=str(path),
                fact_type="note",
                text=f"Hermes session metadata: model={model}; platform={platform}.",
                scenario="session_metadata",
                raw_id=session_id,
            )
        seen_convention = False
        for idx, msg in enumerate(session.get("messages") or []):
            role = str(msg.get("role") or "")
            content = str(msg.get("content") or "")
            raw_id = f"{session_id}:m{idx}"
            if role == "user":
                if "Be concise. Don't guess" in content and not seen_convention:
                    seen_convention = True
                    add(
                        rows,
                        dataset="hermes",
                        source=str(path),
                        fact_type="convention",
                        text="User instruction for the Hermes session: be concise and do not guess.",
                        scenario="user_instruction",
                        raw_id=raw_id,
                    )
                match = QUESTION_RE.search(content)
                if match:
                    add(
                        rows,
                        dataset="hermes",
                        source=str(path),
                        fact_type="question",
                        text=f"Question: {compact(match.group(1), 300)}",
                        scenario="user_question",
                        raw_id=raw_id,
                    )
            for call_idx, call in enumerate(msg.get("tool_calls") or []):
                function = call.get("function") or {}
                name = compact(function.get("name") or call.get("name") or "")
                args = compact(function.get("arguments") or call.get("arguments") or {})
                if name:
                    add(
                        rows,
                        dataset="hermes",
                        source=str(path),
                        fact_type="decision",
                        text=f"Hermes tool call: {name} with arguments {args}.",
                        scenario="tool_call",
                        raw_id=f"{raw_id}:call{call_idx}",
                    )
            if role == "tool":
                is_error = False
                try:
                    parsed_content = json.loads(content)
                    if isinstance(parsed_content, dict):
                        is_error = (
                            parsed_content.get("success") is False
                            or bool(parsed_content.get("error"))
                            or bool(parsed_content.get("error_message"))
                        )
                except Exception:
                    is_error = False
                if is_error:
                    add(
                        rows,
                        dataset="hermes",
                        source=str(path),
                        fact_type="error",
                        text=f"Hermes tool result reported an error: {compact(content, 320)}",
                        scenario="tool_error",
                        raw_id=raw_id,
                    )
                else:
                    add(
                        rows,
                        dataset="hermes",
                        source=str(path),
                        fact_type="note",
                        text=f"Hermes tool evidence: {compact(content, 320)}",
                        scenario="tool_evidence",
                        raw_id=raw_id,
                    )
    return rows


def balanced(rows: list[dict[str, Any]], max_rows: int, max_per_label: int) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    seen_text: set[str] = set()
    counts: Counter[str] = Counter()
    by_label: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        key = row["text"].lower()
        if key in seen_text:
            continue
        seen_text.add(key)
        by_label[row["fact_type"]].append(row)
    while len(out) < max_rows:
        progressed = False
        for label in LABELS:
            if counts[label] >= max_per_label:
                continue
            bucket = by_label.get(label) or []
            if counts[label] < len(bucket):
                row = dict(bucket[counts[label]])
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
    parser.add_argument(
        "--agentstep-root",
        type=Path,
        default=Path(os.environ.get("AGENTSTEP_ROOT", Path.home() / "AgentStep")),
    )
    parser.add_argument(
        "--hermes-sessions-dir",
        type=Path,
        default=Path(
            os.environ.get(
                "HERMES_SESSIONS_DIR",
                Path.home() / ".hermes" / "sessions",
            )
        ),
    )
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--max-rows", type=int, default=72)
    parser.add_argument("--max-per-label", type=int, default=12)
    args = parser.parse_args()

    datasets = {
        "agentstep": balanced(agentstep_rows(args.agentstep_root), args.max_rows, args.max_per_label),
        "hermes": balanced(hermes_rows(args.hermes_sessions_dir), args.max_rows, args.max_per_label),
    }
    combined: list[dict[str, Any]] = []
    for name, rows in datasets.items():
        path = args.out_dir / f"{name}_fact_type_probe.jsonl"
        write_jsonl(path, rows)
        combined.extend(rows)
    combined = balanced(combined, args.max_rows * len(datasets), args.max_per_label * len(datasets))
    write_jsonl(args.out_dir / "combined_fact_type_probe.jsonl", combined)

    summary = {
        name: {
            "rows": len(rows),
            "distribution": dict(Counter(row["fact_type"] for row in rows)),
        }
        for name, rows in {**datasets, "combined": combined}.items()
    }
    print(json.dumps(summary, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
