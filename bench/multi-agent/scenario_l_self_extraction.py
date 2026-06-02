#!/usr/bin/env python3
"""Scenario L - self-extracted facts drive workflow memory.

aidememo deliberately keeps LLM extraction outside the storage layer: the calling
agent should classify facts, then persist them through aidememo_fact_add_many. This
zero-token scenario validates that contract without calling an LLM:

  1. Simulate an agent's classified output from a short project transcript.
  2. Insert the batch through MCP aidememo_fact_add_many in one transaction.
  3. Verify fact_type/source_id persistence.
  4. Start sparse tickets and check decisions / lessons / errors surface while
     neighbouring source_id facts do not leak.

The scenario is not a classifier benchmark. It proves that if the agent follows
the self-extraction prompt table, aidememo stores and retrieves the typed memory in
the workflow shape coding agents consume.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from collections import Counter
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
WG = os.environ.get("AIDEMEMO_BIN", str(REPO / "target" / "debug" / "aidememo"))
STORE = os.environ.get(
    "AIDEMEMO_E2E_STORE",
    str(Path(tempfile.gettempdir()) / "aidememo-e2e-l" / "self-extraction.redb"),
)

SELF_EXTRACTED_FACTS: list[dict[str, Any]] = [
    {
        "content": "Decision: Billing export dedupe uses Postgres advisory locks keyed by invoice id.",
        "fact_type": "decision",
        "entities": ["BillingExport", "Postgres"],
        "source_id": "agent-alpha",
    },
    {
        "content": "Lesson: BullMQ retries caused duplicate billing exports when idempotency keys were missing.",
        "fact_type": "lesson",
        "entities": ["BillingExport", "BullMQ"],
        "source_id": "agent-alpha",
    },
    {
        "content": "Error: Do not disable webhook signature validation while debugging billing duplicates.",
        "fact_type": "error",
        "entities": ["BillingWebhook", "Stripe"],
        "source_id": "agent-alpha",
    },
    {
        "content": "Preference: PR summaries should include risk, rollback, and verification sections.",
        "fact_type": "preference",
        "entities": ["PullRequest"],
        "source_id": "agent-alpha",
    },
    {
        "content": "Convention: Billing export fixes must include an idempotency regression test.",
        "fact_type": "convention",
        "entities": ["BillingExport", "Tests"],
        "source_id": "agent-alpha",
    },
    {
        "content": "Decision: Edge cache dedupe uses DynamoDB conditional writes for CDN events.",
        "fact_type": "decision",
        "entities": ["EdgeCache", "DynamoDB"],
        "source_id": "agent-beta",
    },
    {
        "content": "Lesson: CDN event duplication came from stale edge routing rules.",
        "fact_type": "lesson",
        "entities": ["EdgeCache", "CDN"],
        "source_id": "agent-beta",
    },
]


def run(
    cmd: list[str],
    *,
    input_text: str | None = None,
    env: dict[str, str] | None = None,
    timeout: int = 30,
) -> subprocess.CompletedProcess:
    child_env = os.environ.copy()
    if env:
        child_env.update(env)
    proc = subprocess.run(
        cmd,
        input=input_text,
        capture_output=True,
        text=True,
        env=child_env,
        timeout=timeout,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{cmd!r} exited {proc.returncode}\n"
            f"stdout={proc.stdout[:1000]}\nstderr={proc.stderr[:1600]}"
        )
    return proc


def reset_store() -> None:
    path = Path(STORE)
    path.parent.mkdir(parents=True, exist_ok=True)
    for sibling in path.parent.iterdir():
        if sibling.name.startswith(path.name):
            if sibling.is_dir():
                shutil.rmtree(sibling)
            else:
                sibling.unlink()


def mcp_tool_call(
    name: str,
    args: dict[str, Any],
    *,
    env: dict[str, str] | None = None,
) -> dict[str, Any]:
    calls = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}},
        },
        {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": args},
        },
    ]
    proc = run(
        [WG, "--store", STORE, "mcp"],
        input_text="".join(json.dumps(call) + "\n" for call in calls),
        env=env,
    )
    responses = [
        json.loads(line) for line in proc.stdout.splitlines() if line.strip().startswith("{")
    ]
    by_id = {response.get("id"): response for response in responses}
    response = by_id.get(2) or {}
    if "error" in response:
        raise RuntimeError(f"MCP {name} failed: {response['error']}")
    content = response.get("result", {}).get("content") or []
    if not content:
        raise RuntimeError(f"MCP {name} returned no content: {response}")
    return json.loads(content[0]["text"])


def fact_list(source_id: str) -> list[dict[str, Any]]:
    payload = json.loads(
        run(
            [
                WG,
                "--store",
                STORE,
                "--json",
                "fact",
                "list",
                "--source-id",
                source_id,
                "--limit",
                "50",
            ]
        ).stdout
    )
    return payload


def workflow_start(title: str, body: str, source_id: str) -> tuple[dict[str, Any], float]:
    start = time.perf_counter_ns()
    payload = json.loads(
        run(
            [
                WG,
                "--store",
                STORE,
                "--json",
                "workflow",
                "start",
                title,
                "--body",
                body,
                "--source",
                f"scenario-l:{source_id}",
                "--source-id",
                source_id,
                "--limit",
                "10",
                "--depth",
                "2",
                "--recent-limit",
                "5",
                "--bm25-only",
            ]
        ).stdout
    )
    return payload, (time.perf_counter_ns() - start) / 1e6


def search(topic: str, source_id: str) -> list[dict[str, Any]]:
    return json.loads(
        run(
            [
                WG,
                "--store",
                STORE,
                "--json",
                "search",
                topic,
                "--source-id",
                source_id,
                "--limit",
                "10",
            ]
        ).stdout
    )


def contains(rows: list[dict[str, Any]], needle: str) -> bool:
    return any(needle in row.get("content", "") for row in rows)


def workflow_summary(payload: dict[str, Any], latency_ms: float) -> dict[str, Any]:
    text = json.dumps(payload, ensure_ascii=False)
    return {
        "session_id_present": str(payload.get("session_id", "")).startswith("session-"),
        "ticket_fact_id_present": isinstance(payload.get("ticket_fact_id"), str),
        "source_id": payload.get("source_id"),
        "latency_ms": round(latency_ms, 2),
        "search_hits": len((payload.get("context") or {}).get("search") or []),
        "relevant_decisions": len(payload.get("relevant_decisions") or []),
        "prior_lessons": len(payload.get("prior_lessons") or []),
        "prior_errors": len(payload.get("prior_errors") or []),
        "contains": {
            "advisory_locks": "Postgres advisory locks" in text,
            "bullmq_retries": "BullMQ retries" in text,
            "signature_validation": "webhook signature validation" in text,
            "dynamodb": "DynamoDB conditional writes" in text,
            "cdn": "CDN event duplication" in text,
        },
        "beta_source_markers": {
            "dynamodb": "DynamoDB conditional writes" in text,
            "cdn": "CDN event duplication" in text,
        },
    }


def main() -> int:
    reset_store()

    add_start = time.perf_counter_ns()
    add_payload = mcp_tool_call("aidememo_fact_add_many", {"items": SELF_EXTRACTED_FACTS})
    add_latency_ms = (time.perf_counter_ns() - add_start) / 1e6
    env_default_payload = mcp_tool_call(
        "aidememo_fact_add_many",
        {
            "items": [
                {
                    "content": "Decision: source defaults smoke uses AIDEMEMO_SOURCE_ID for MCP scoping.",
                    "fact_type": "decision",
                    "entities": ["SourceDefaults"],
                }
            ]
        },
        env={"AIDEMEMO_SOURCE_ID": "agent-env"},
    )
    env_search_payload = mcp_tool_call(
        "aidememo_search",
        {"query": "source defaults MCP scoping", "bm25_only": True, "limit": 5},
        env={"AIDEMEMO_SOURCE_ID": "agent-env"},
    )

    alpha_facts = fact_list("agent-alpha")
    beta_facts = fact_list("agent-beta")
    env_facts = fact_list("agent-env")
    alpha_type_counts = Counter(fact.get("fact_type") for fact in alpha_facts)
    beta_type_counts = Counter(fact.get("fact_type") for fact in beta_facts)

    billing_pack, billing_latency = workflow_start(
        "Fix duplicate billing export job",
        "Exports sometimes run twice after queue retries. The ticket has no implementation detail.",
        "agent-alpha",
    )
    beta_pack, beta_latency = workflow_start(
        "Fix duplicate edge cache events",
        "CDN event processing occasionally records duplicates after edge routing changes.",
        "agent-beta",
    )

    preference_hits = search("PR summary risk rollback verification", "agent-alpha")
    preference_hit_types = [hit.get("fact_type") for hit in preference_hits]

    billing_summary = workflow_summary(billing_pack, billing_latency)
    beta_summary = workflow_summary(beta_pack, beta_latency)

    invariants = {
        "mcp_batch_inserted_all": add_payload.get("count") == len(SELF_EXTRACTED_FACTS),
        "alpha_type_counts": dict(alpha_type_counts)
        == {
            "decision": 1,
            "lesson": 1,
            "error": 1,
            "preference": 1,
            "convention": 1,
        },
        "beta_type_counts": dict(beta_type_counts) == {"decision": 1, "lesson": 1},
        "env_default_source_id_scopes_batch": env_default_payload.get("count") == 1
        and contains(env_facts, "AIDEMEMO_SOURCE_ID for MCP scoping"),
        "env_default_source_id_scopes_search": contains(
            env_search_payload.get("results", []), "AIDEMEMO_SOURCE_ID for MCP scoping"
        ),
        "billing_workflow_has_session": billing_summary["session_id_present"]
        and billing_summary["ticket_fact_id_present"],
        "billing_workflow_recovers_typed_priors": all(
            billing_summary["contains"][key]
            for key in ("advisory_locks", "bullmq_retries", "signature_validation")
        ),
        "billing_workflow_no_beta_leakage": not any(
            billing_summary["beta_source_markers"].values()
        ),
        "beta_workflow_is_scoped": beta_summary["source_id"] == "agent-beta"
        and beta_summary["contains"]["dynamodb"]
        and not beta_summary["contains"]["advisory_locks"],
        "preference_search_surfaces_preference": contains(preference_hits, "PR summaries should include")
        and "preference" in preference_hit_types,
    }

    out = {
        "scenario": "L - self-extracted facts drive workflow memory",
        "store": STORE,
        "insert": {
            "latency_ms": round(add_latency_ms, 2),
            "payload": add_payload,
            "env_default_payload": env_default_payload,
            "env_search_payload": env_search_payload,
        },
        "fact_type_counts": {
            "agent-alpha": dict(alpha_type_counts),
            "agent-beta": dict(beta_type_counts),
            "agent-env": dict(Counter(fact.get("fact_type") for fact in env_facts)),
        },
        "workflow": {
            "agent-alpha": billing_summary,
            "agent-beta": beta_summary,
        },
        "preference_search": {
            "hit_count": len(preference_hits),
            "fact_types": preference_hit_types,
            "top_content": preference_hits[0].get("content") if preference_hits else None,
        },
        "invariants": invariants,
        "summary": {
            "passed": sum(1 for ok in invariants.values() if ok),
            "total": len(invariants),
        },
    }
    out_path = REPO / "bench" / "multi-agent" / "results" / "scenario_l.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, ensure_ascii=False))
    print(json.dumps(out, indent=2, ensure_ascii=False))
    return 0 if out["summary"]["passed"] == out["summary"]["total"] else 1


if __name__ == "__main__":
    sys.exit(main())
