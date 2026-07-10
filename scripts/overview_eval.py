#!/usr/bin/env python3
"""
Phase 1 — does `aidememo_overview` actually help an LLM agent?

A/B compares an agent that has aidememo_overview against one that doesn't, on
a fixed set of tasks against the same fixture wiki. Measures:

  - tool calls per task
  - input + output tokens (proxy for cost)
  - LLM-judge completeness/accuracy score

Usage:
  set -a; source .env; set +a
  ./scripts/overview_eval.py --build --run

  # Just rebuild the fixture (no API calls):
  ./scripts/overview_eval.py --build

  # Run only a subset of tasks:
  ./scripts/overview_eval.py --run --tasks broad_orientation,domain_map
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
AIDEMEMO_BIN = os.environ.get("AIDEMEMO_BIN", str(ROOT / "target" / "debug" / "aidememo"))
STORE = Path(os.environ.get("AIDEMEMO_OVERVIEW_EVAL_STORE", str(Path(tempfile.gettempdir()) / "aidememo_overview_eval.sqlite")))
MODEL = os.environ.get("AIDEMEMO_OVERVIEW_EVAL_MODEL", "gpt-4o")
JUDGE_MODEL = os.environ.get("AIDEMEMO_OVERVIEW_EVAL_JUDGE", "gpt-4o")
MAX_TOOL_TURNS = 10

# ──────────────────────────────────────────────────────────── fixture spec

ENTITIES: list[tuple[str, str]] = [
    # Data Layer
    ("Postgres", "technology"),
    ("Redis", "technology"),
    ("Kafka", "technology"),
    ("ClickHouse", "technology"),
    ("S3", "technology"),
    # Auth
    ("Keycloak", "technology"),
    ("JWT-Service", "service"),
    ("OAuth-Provider", "service"),
    ("Session-Store", "service"),
    ("Auth-RFC-12", "rfc"),
    # Frontend
    ("NextJS-App", "service"),
    ("Tailwind", "technology"),
    ("Vercel", "technology"),
    ("React-Components", "concept"),
    ("CDN-Edge", "service"),
    # Observability
    ("Datadog", "technology"),
    ("PagerDuty", "technology"),
    ("Sentry", "technology"),
    ("Grafana", "technology"),
    ("Loki", "technology"),
    # People
    ("Alice", "person"),
    ("Bob", "person"),
    ("Carol", "person"),
    ("Dave", "person"),
    ("Eve", "person"),
    # Incidents
    ("Outage-2026-04-12", "incident"),
    ("Outage-2026-04-22", "incident"),
]

# (fact_type, content, [entity names])
FACTS: list[tuple[str, str, list[str]]] = [
    # data layer
    ("decision", "Use Postgres 16 as the primary OLTP database, with read replicas for reporting.", ["Postgres"]),
    ("decision", "Redis Cluster handles the session and cache layer for the auth and frontend.", ["Redis", "Session-Store"]),
    ("decision", "Kafka topics partition by tenant_id; retention is 7 days for raw events, 90 days for compactions.", ["Kafka"]),
    ("decision", "ClickHouse is the analytics warehouse — Kafka events flow into it via Kafka Connect.", ["ClickHouse", "Kafka"]),
    ("decision", "All long-term blob storage goes to S3 with intelligent tiering after 30 days.", ["S3"]),
    ("pattern", "Postgres uses logical replication for CDC into ClickHouse via Debezium.", ["Postgres", "ClickHouse"]),
    ("convention", "Postgres column names are snake_case; primary keys are always `id` (uuid v7).", ["Postgres"]),
    ("note", "Replica lag spiked to 8 minutes on 2026-04-12; root cause was a runaway analytics query.", ["Postgres", "Outage-2026-04-12"]),
    ("claim", "Redis is roughly 30x faster than Postgres for hot read paths under our load profile.", ["Redis", "Postgres"]),

    # auth
    ("decision", "Keycloak is our IdP — it sits behind the OAuth-Provider façade; everything else is OIDC.", ["Keycloak", "OAuth-Provider"]),
    ("decision", "JWT-Service issues short-lived (15 min) access tokens and long-lived (30 day) refresh tokens.", ["JWT-Service"]),
    ("decision", "Auth-RFC-12 mandates rotating signing keys every 30 days, automated via Vault.", ["Auth-RFC-12", "JWT-Service"]),
    ("pattern", "Session-Store is Redis-backed; expiry mirrors the refresh-token lifetime.", ["Session-Store", "Redis"]),
    ("note", "Alice is the auth domain owner; sign-off on Auth-RFC-* changes goes through her.", ["Alice", "Auth-RFC-12"]),
    ("note", "OAuth-Provider rate limit lifted from 100 → 500 rps on 2026-04-15 after the Vercel migration.", ["OAuth-Provider", "Vercel"]),

    # frontend
    ("decision", "NextJS-App runs on Vercel edge; ISR for product pages, SSR for dashboards.", ["NextJS-App", "Vercel"]),
    ("decision", "Tailwind is the only allowed styling system — no CSS modules or styled-components.", ["Tailwind"]),
    ("convention", "React-Components are organised feature-first; primitives live in /components/ui.", ["React-Components"]),
    ("pattern", "CDN-Edge serves /static and /api/edge/*; origin only sees authenticated /api/* traffic.", ["CDN-Edge", "Vercel"]),
    ("note", "Bob owns the frontend platform; on-call rotation includes Bob and Carol.", ["Bob", "Carol", "NextJS-App"]),

    # observability
    ("decision", "Datadog is the primary metrics + APM tool; PagerDuty for alerts.", ["Datadog", "PagerDuty"]),
    ("decision", "Sentry captures frontend + backend exceptions; backend SDK is wired into NextJS-App and JWT-Service.", ["Sentry", "NextJS-App", "JWT-Service"]),
    ("pattern", "Grafana dashboards pull from Loki for logs and Datadog for metrics — no other source allowed.", ["Grafana", "Loki", "Datadog"]),
    ("convention", "All services emit OpenTelemetry traces; trace IDs propagate via W3C TraceContext.", ["Datadog"]),
    ("note", "Carol is the observability lead; she rotates the on-call schedule weekly.", ["Carol"]),
    ("note", "PagerDuty integration with Datadog flapped on 2026-04-22 — we missed a JWT-Service 503 spike.", ["PagerDuty", "Datadog", "JWT-Service", "Outage-2026-04-22"]),

    # people / cross-team
    ("note", "Dave runs the data platform team; he owns Kafka, ClickHouse, and the Debezium CDC pipeline.", ["Dave", "Kafka", "ClickHouse"]),
    ("note", "Eve is the security lead; she signs off on Vault rotation policy.", ["Eve", "Auth-RFC-12"]),

    # cross-topic links
    ("decision", "Auth tokens are cached in Redis Session-Store; cache miss falls through to JWT-Service which validates against Keycloak.", ["Redis", "Session-Store", "JWT-Service", "Keycloak"]),
    ("note", "NextJS-App hits OAuth-Provider directly for login; once authenticated, /api calls carry JWTs validated by CDN-Edge.", ["NextJS-App", "OAuth-Provider", "CDN-Edge", "JWT-Service"]),

    # questions / open
    ("question", "Should we move Session-Store off Redis Cluster onto Redis Standalone with persistence? Carol is investigating.", ["Session-Store", "Redis", "Carol"]),
    ("question", "Is ClickHouse appropriate for sub-second dashboards or should we add Druid?", ["ClickHouse"]),

    # claims
    ("claim", "Vercel egress costs us ~$2k/month at current traffic; 3x-ing would push us to enterprise tier.", ["Vercel"]),
    ("claim", "Keycloak's admin UI is the slowest piece of our auth stack — single largest support-ticket source.", ["Keycloak"]),
]


@dataclass
class Task:
    id: str
    prompt: str
    # Lower-cased substrings the answer should ideally mention.
    must_mention: list[str] = field(default_factory=list)
    # Minimum count of must_mention substrings expected.
    min_must_mention: int = 0
    # Free-form rubric clue for the judge (one sentence).
    rubric_hint: str = ""


TASKS: list[Task] = [
    Task(
        id="broad_orientation",
        prompt=(
            "I'm new to this codebase. Give me a 1-paragraph map of the major topic "
            "areas in this wiki and what each one is about. List the topic names you "
            "identify (3-5 labels) and the key entities in each."
        ),
        must_mention=["data", "auth", "frontend", "observability"],
        min_must_mention=3,
        rubric_hint="A correct answer names 3+ distinct topic groups (e.g. data layer / auth / frontend / observability) and lists representative entities for each.",
    ),
    Task(
        id="domain_map",
        prompt="What domain areas does this wiki cover? Just name the high-level groupings, no per-fact details.",
        must_mention=["data", "auth", "frontend", "observability", "people"],
        min_must_mention=3,
        rubric_hint="A correct answer is a short list of 3-5 distinct topic labels.",
    ),
    Task(
        id="data_layer_decisions",
        prompt="Summarize the data-layer decisions in this wiki: which databases / queues / storage we use and why.",
        must_mention=["postgres", "redis", "kafka", "clickhouse", "s3"],
        min_must_mention=4,
        rubric_hint="A correct answer lists at least 4 of: Postgres, Redis, Kafka, ClickHouse, S3 — with their roles.",
    ),
    Task(
        id="postgres_facts",
        prompt="What facts do we have about Postgres specifically? List them.",
        must_mention=["postgres", "snake_case", "replica", "uuid"],
        min_must_mention=2,
        rubric_hint="A correct answer surfaces specific Postgres facts: snake_case convention, replica lag incident, OLTP role, debezium CDC.",
    ),
    Task(
        id="people_overview",
        prompt="Who are the people in this wiki and what do they own?",
        must_mention=["alice", "bob", "carol", "dave", "eve"],
        min_must_mention=4,
        rubric_hint="A correct answer names 4-5 of Alice/Bob/Carol/Dave/Eve and pairs each with their domain.",
    ),
    Task(
        id="cross_topic",
        prompt="How does the auth system intersect with the data layer in this wiki? Describe the connection.",
        must_mention=["redis", "session", "jwt", "keycloak"],
        min_must_mention=2,
        rubric_hint="A correct answer mentions Redis-backed Session-Store + JWT-Service + Keycloak as the auth↔data bridge.",
    ),
    Task(
        id="recent_activity",
        prompt="What was added or changed recently in this wiki? Just the high-level recent activity.",
        must_mention=[],
        min_must_mention=0,
        rubric_hint="A correct answer summarises the recent fact list (last 7 days) — most facts in this fixture are recent so the answer should mention several entries.",
    ),
]


# ──────────────────────────────────────────────────────────── fixture build

def run(cmd: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, check=True)


def build_fixture() -> None:
    if STORE.exists():
        STORE.unlink()
    sidecar = STORE.with_suffix(".hnsw.bin")
    if sidecar.exists():
        sidecar.unlink()

    for name, etype in ENTITIES:
        run([AIDEMEMO_BIN, "--store", str(STORE), "entity", "add", name, "--type", etype])

    for ftype, content, entities in FACTS:
        run([
            AIDEMEMO_BIN, "--store", str(STORE),
            "fact", "add", content,
            "--type", ftype,
            "--entities", ",".join(entities),
        ])

    print(f"fixture built: {len(ENTITIES)} entities, {len(FACTS)} facts → {STORE}")
    overview = run([AIDEMEMO_BIN, "--store", str(STORE), "overview"])
    print()
    print(overview.stdout)


# ──────────────────────────────────────────────────────────── MCP client

class AideMemoMCP:
    """Long-lived `aidememo mcp` stdio session."""

    def __init__(self, store: Path) -> None:
        self.proc = subprocess.Popen(
            [AIDEMEMO_BIN, "--store", str(store), "mcp"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        self._next_id = 1
        # initialize handshake
        self._send({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "overview-eval", "version": "0"},
            },
        })
        self._recv()

    def _send(self, obj: dict) -> None:
        assert self.proc.stdin
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()

    def _recv(self) -> dict | None:
        assert self.proc.stdout
        line = self.proc.stdout.readline()
        if not line:
            return None
        return json.loads(line)

    def call(self, tool: str, args: dict) -> str:
        self._send({
            "jsonrpc": "2.0", "id": self._next_id,
            "method": "tools/call",
            "params": {"name": tool, "arguments": args},
        })
        self._next_id += 1
        resp = self._recv()
        if not resp:
            return "<no response>"
        if "error" in resp:
            return f"Error: {resp['error']}"
        result = resp.get("result", {})
        content = result.get("content", [])
        if content and isinstance(content, list) and content[0].get("type") == "text":
            return content[0]["text"]
        return json.dumps(result)

    def close(self) -> None:
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


# ──────────────────────────────────────────────────────────── tool schemas

# Subset of aidememo's MCP tools — manually curated so the OpenAI tool-calling
# format matches what `aidememo mcp tools/list` would return.
def tool_search() -> dict:
    return {
        "type": "function",
        "function": {
            "name": "aidememo_search",
            "description": "Hybrid BM25 + semantic search over facts. Returns ranked hits.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "number", "default": 10},
                    "entity": {"type": "string", "description": "Restrict to facts attached to this entity."},
                },
                "required": ["query"],
            },
        },
    }


def tool_query() -> dict:
    return {
        "type": "function",
        "function": {
            "name": "aidememo_query",
            "description": "Unified context fetch — combines hybrid search, entity resolution, traversal, and recent facts in one call. Pass a topic or entity name.",
            "parameters": {
                "type": "object",
                "properties": {
                    "topic": {"type": "string"},
                    "limit": {"type": "number", "default": 10},
                    "depth": {"type": "number", "default": 2},
                    "recent_limit": {"type": "number", "default": 10},
                },
                "required": ["topic"],
            },
        },
    }


def tool_traverse() -> dict:
    return {
        "type": "function",
        "function": {
            "name": "aidememo_traverse",
            "description": "Forward graph walk from an entity. Returns reachable entities up to depth.",
            "parameters": {
                "type": "object",
                "properties": {
                    "entity": {"type": "string"},
                    "depth": {"type": "number", "default": 2},
                },
                "required": ["entity"],
            },
        },
    }


def tool_recent() -> dict:
    return {
        "type": "function",
        "function": {
            "name": "aidememo_recent",
            "description": "Recently added/updated facts. Defaults to last 7 days.",
            "parameters": {
                "type": "object",
                "properties": {
                    "limit": {"type": "number", "default": 20},
                    "last_days": {"type": "number", "default": 7},
                },
            },
        },
    }


def tool_entity_list() -> dict:
    return {
        "type": "function",
        "function": {
            "name": "aidememo_entity_list",
            "description": "List entities in the aidememo with fact counts.",
            "parameters": {
                "type": "object",
                "properties": {
                    "limit": {"type": "number", "default": 50},
                    "type": {"type": "string", "description": "Filter by entity type"},
                },
            },
        },
    }


def tool_fact_list() -> dict:
    return {
        "type": "function",
        "function": {
            "name": "aidememo_fact_list",
            "description": "List facts with optional entity filter.",
            "parameters": {
                "type": "object",
                "properties": {
                    "limit": {"type": "number", "default": 50},
                    "entity": {"type": "string"},
                },
            },
        },
    }


def tool_entity_get() -> dict:
    return {
        "type": "function",
        "function": {
            "name": "aidememo_entity_get",
            "description": "Get one entity by name or alias.",
            "parameters": {
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"],
            },
        },
    }


def tool_overview() -> dict:
    return {
        "type": "function",
        "function": {
            "name": "aidememo_overview",
            "description": (
                "First-impression snapshot of the wiki: entity-type buckets with top "
                "examples, fact-type distribution, top central entities by fact_count, "
                "recent activity, and current/pinned/orphan counts. Designed for an "
                "agent arriving at an unfamiliar wiki — one call instead of stats + "
                "entity_list + fact_list."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "top_n": {"type": "number", "default": 10},
                    "recent_days": {"type": "number", "default": 7},
                },
            },
        },
    }


TOOLS_BASE = [
    tool_search(), tool_query(), tool_traverse(), tool_recent(),
    tool_entity_list(), tool_fact_list(), tool_entity_get(),
]
TOOLS_WITH_OVERVIEW = TOOLS_BASE + [tool_overview()]


# ──────────────────────────────────────────────────────────── agent driver

SYSTEM_PROMPT = """You are an agent inspecting a knowledge-graph wiki named `aidememo`. Use the provided tools to gather the information you need, then write a concise, well-organised answer to the user's question. Budget: at most {max_turns} tool calls. Stop calling tools as soon as you have enough information; don't pad."""


def _openai_request(api_key: str, body: dict, timeout: int = 60) -> dict:
    url = "https://api.openai.com/v1/chat/completions"
    last_err: Exception | None = None
    for attempt in range(4):
        req = urllib.request.Request(
            url,
            data=json.dumps(body).encode(),
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return json.loads(resp.read().decode())
        except urllib.error.HTTPError as e:
            body_txt = e.read().decode("utf-8", errors="replace")
            if e.code in (429, 500, 502, 503, 504):
                wait = [2, 5, 12, 30][attempt]
                print(f"  [retry {attempt+1}/4] HTTP {e.code} — sleeping {wait}s", file=sys.stderr)
                time.sleep(wait)
                last_err = RuntimeError(f"HTTP {e.code}: {body_txt[:200]}")
                continue
            raise RuntimeError(f"HTTP {e.code}: {body_txt[:200]}")
        except (urllib.error.URLError, TimeoutError) as e:
            wait = [2, 5, 12, 30][attempt]
            print(f"  [retry {attempt+1}/4] {e} — sleeping {wait}s", file=sys.stderr)
            time.sleep(wait)
            last_err = e
            continue
    raise RuntimeError(f"all retries failed: {last_err}")


def run_agent(mcp: AideMemoMCP, tools: list[dict], task: Task, model: str) -> dict:
    api_key = os.environ["OPENAI_API_KEY"]
    messages: list[dict] = [
        {"role": "system", "content": SYSTEM_PROMPT.format(max_turns=MAX_TOOL_TURNS)},
        {"role": "user", "content": task.prompt},
    ]
    tool_calls_log: list[dict] = []
    tin = 0
    tout = 0

    for _turn in range(MAX_TOOL_TURNS + 1):
        resp = _openai_request(api_key, {
            "model": model,
            "messages": messages,
            "tools": tools,
        })
        usage = resp.get("usage") or {}
        tin += usage.get("prompt_tokens", 0) or 0
        tout += usage.get("completion_tokens", 0) or 0
        msg = resp["choices"][0]["message"]
        tool_calls = msg.get("tool_calls") or []

        if tool_calls:
            # Echo the assistant turn so the API has the matching
            # tool_call_id when we send back the tool results.
            messages.append({
                "role": "assistant",
                "content": msg.get("content") or "",
                "tool_calls": [
                    {
                        "id": tc["id"],
                        "type": "function",
                        "function": {
                            "name": tc["function"]["name"],
                            "arguments": tc["function"].get("arguments", "{}"),
                        },
                    }
                    for tc in tool_calls
                ],
            })
            for tc in tool_calls:
                fn = tc["function"]
                try:
                    args = json.loads(fn.get("arguments") or "{}")
                except json.JSONDecodeError:
                    args = {}
                result = mcp.call(fn["name"], args)
                tool_calls_log.append({
                    "name": fn["name"],
                    "args": args,
                    "result_chars": len(result),
                })
                # Cap tool result size to keep the conversation small;
                # 6 KB is generous and matches what aidememo returns for the
                # typical query / overview call.
                if len(result) > 6000:
                    result = result[:6000] + "\n…<truncated>"
                messages.append({
                    "role": "tool",
                    "tool_call_id": tc["id"],
                    "content": result,
                })
        else:
            return {
                "answer": msg.get("content") or "",
                "tool_calls": tool_calls_log,
                "tokens_in": tin,
                "tokens_out": tout,
            }

    return {
        "answer": "<turn budget exhausted>",
        "tool_calls": tool_calls_log,
        "tokens_in": tin,
        "tokens_out": tout,
    }


# ──────────────────────────────────────────────────────────── judge

JUDGE_PROMPT = """You are evaluating an LLM agent's answer to a question about a knowledge-graph wiki.

QUESTION:
{prompt}

RUBRIC HINT: {rubric_hint}
Items the answer should ideally mention (lower-cased substrings, case-insensitive match): {must_mention}
Minimum count of must-mention items expected: {min_must}

ANSWER:
\"\"\"
{answer}
\"\"\"

Score the answer and output JSON with these fields:
{{
  "completeness": <0-100, how thoroughly the answer covers what was asked>,
  "groundedness": <0-100, are the claims plausibly drawn from a wiki, not hallucinated>,
  "must_mention_hit": <integer count of must-mention substrings actually present>,
  "rationale": "<one sentence>"
}}

Output ONLY the JSON object, no commentary."""


def judge(task: Task, answer: str, model: str) -> dict:
    api_key = os.environ["OPENAI_API_KEY"]
    prompt = JUDGE_PROMPT.format(
        prompt=task.prompt,
        rubric_hint=task.rubric_hint,
        must_mention=", ".join(task.must_mention) or "(none required)",
        min_must=task.min_must_mention,
        answer=answer,
    )
    resp = _openai_request(api_key, {
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "response_format": {"type": "json_object"},
    })
    return json.loads(resp["choices"][0]["message"]["content"])


# ──────────────────────────────────────────────────────────── main

def main() -> int:
    p = argparse.ArgumentParser(description="Phase 1 aidememo_overview A/B eval.")
    p.add_argument("--build", action="store_true", help="Rebuild the fixture wiki")
    p.add_argument("--run", action="store_true", help="Run the A/B simulation")
    p.add_argument("--tasks", type=str, default="", help="Comma-separated task ids; default = all")
    p.add_argument("--out", type=Path, default=Path("/tmp/aidememo_overview_eval_results.json"))
    p.add_argument("--repeat", type=int, default=1, help="Repeat each (task, condition) N times — picks the median")
    args = p.parse_args()

    if args.build:
        build_fixture()
    if not STORE.exists():
        print(f"error: fixture store not found at {STORE} — run with --build first.", file=sys.stderr)
        return 2
    if not args.run:
        return 0

    if not os.environ.get("OPENAI_API_KEY"):
        print("error: OPENAI_API_KEY not set.", file=sys.stderr)
        return 2

    selected: list[Task] = TASKS
    if args.tasks:
        wanted = {t.strip() for t in args.tasks.split(",") if t.strip()}
        selected = [t for t in TASKS if t.id in wanted]
        if not selected:
            print(f"error: no tasks matched {wanted}", file=sys.stderr)
            return 2

    print(f"running {len(selected)} task(s) × 2 conditions × {args.repeat} repeat(s) = {len(selected) * 2 * args.repeat} agent runs")
    print(f"  model:       {MODEL}")
    print(f"  judge:       {JUDGE_MODEL}")
    print(f"  store:       {STORE}")
    print()

    results: list[dict] = []
    for task in selected:
        for condition in ("A", "B"):
            tools = TOOLS_BASE if condition == "A" else TOOLS_WITH_OVERVIEW
            for rep in range(args.repeat):
                mcp = AideMemoMCP(STORE)
                t0 = time.time()
                try:
                    out = run_agent(mcp, tools, task, MODEL)
                finally:
                    mcp.close()
                elapsed = time.time() - t0
                score = judge(task, out["answer"], JUDGE_MODEL)
                row = {
                    "task_id": task.id,
                    "condition": condition,
                    "rep": rep,
                    "tool_calls": [tc["name"] for tc in out["tool_calls"]],
                    "n_tool_calls": len(out["tool_calls"]),
                    "tokens_in": out["tokens_in"],
                    "tokens_out": out["tokens_out"],
                    "elapsed_s": round(elapsed, 2),
                    "completeness": score.get("completeness"),
                    "groundedness": score.get("groundedness"),
                    "must_mention_hit": score.get("must_mention_hit"),
                    "rationale": score.get("rationale"),
                    "answer": out["answer"],
                }
                results.append(row)
                print(
                    f"[{task.id:<22}/{condition}/{rep}] "
                    f"calls={row['n_tool_calls']:<2} "
                    f"tin={row['tokens_in']:<6} tout={row['tokens_out']:<5} "
                    f"comp={row['completeness']:<3} hit={row['must_mention_hit']}"
                )

    args.out.write_text(json.dumps(results, indent=2))
    print(f"\nfull results → {args.out}")

    # ── summary ─────────────────────────────────────────────────────────
    print()
    print("=" * 70)
    print("Summary (averaged across repeats):")
    print("=" * 70)
    print(f"{'task':<22} {'A: calls':<10} {'B: calls':<10} {'A: comp':<9} {'B: comp':<9} {'Δ comp':<6}")
    print("-" * 70)
    by_task: dict[str, dict[str, list[dict]]] = {}
    for r in results:
        by_task.setdefault(r["task_id"], {"A": [], "B": []})[r["condition"]].append(r)
    deltas = []
    for tid, conds in by_task.items():
        a = conds["A"]
        b = conds["B"]
        a_calls = sum(r["n_tool_calls"] for r in a) / max(len(a), 1)
        b_calls = sum(r["n_tool_calls"] for r in b) / max(len(b), 1)
        a_comp = sum(r["completeness"] or 0 for r in a) / max(len(a), 1)
        b_comp = sum(r["completeness"] or 0 for r in b) / max(len(b), 1)
        delta = b_comp - a_comp
        deltas.append(delta)
        print(f"{tid:<22} {a_calls:<10.1f} {b_calls:<10.1f} {a_comp:<9.1f} {b_comp:<9.1f} {delta:+.1f}")
    print("-" * 70)
    a_calls_total = sum(r["n_tool_calls"] for r in results if r["condition"] == "A")
    b_calls_total = sum(r["n_tool_calls"] for r in results if r["condition"] == "B")
    a_tin = sum(r["tokens_in"] for r in results if r["condition"] == "A")
    b_tin = sum(r["tokens_in"] for r in results if r["condition"] == "B")
    a_comp = sum(r["completeness"] or 0 for r in results if r["condition"] == "A") / max(sum(1 for r in results if r["condition"] == "A"), 1)
    b_comp = sum(r["completeness"] or 0 for r in results if r["condition"] == "B") / max(sum(1 for r in results if r["condition"] == "B"), 1)
    print(f"TOTAL                  A_calls={a_calls_total:<5} B_calls={b_calls_total:<5} A_tin={a_tin:<7} B_tin={b_tin:<7}")
    print(f"AVG completeness:      A={a_comp:.1f}  B={b_comp:.1f}  (Δ={b_comp - a_comp:+.1f})")
    print(f"AVG completeness Δ per task: {sum(deltas) / max(len(deltas), 1):+.2f}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
