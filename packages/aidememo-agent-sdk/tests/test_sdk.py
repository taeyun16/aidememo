from __future__ import annotations

import json
import sys
import types

from aidememo_agent.client import AideMemoClient
from aidememo_agent.sdk import AideMemoMemorySDK


class FakeClient:
    def __init__(self) -> None:
        self.fact_batches = []
        self.branch_calls = []
        self.artifact_calls = []

    def search(self, query, **kwargs):
        return [
            {
                "fact_id": f"fact-{query}",
                "content": f"{query} result",
                "entity_names": ["Redis"],
                "source_id": kwargs.get("source_id"),
            }
        ]

    def query(self, topic, **kwargs):
        return {"topic": topic, "source_id": kwargs.get("source_id")}

    def aggregate(self, query, **kwargs):
        return {"op": kwargs.get("op"), "query": query, "matched": 1, "source_id": kwargs.get("source_id")}

    def fact_add_many(self, items):
        self.fact_batches.append(items)
        return [f"fact-{idx}" for idx, _ in enumerate(items, start=1)]

    def branch_push(self, branch, destination, *, base=None):
        self.branch_calls.append(("push", branch, destination, base))
        return {
            "branch_id": branch,
            "destination": destination,
            "base": base,
            "records_exported": 1,
        }

    def branch_merge(self, source, *, branch=None):
        self.branch_calls.append(("merge", branch, source))
        return {
            "source": source,
            "branch": branch,
            "segments_merged": 1,
            "facts_inserted": 1,
        }

    def session_canvas(self, session_id=None, *, limit=80, include_superseded=False):
        self.artifact_calls.append(("session_canvas", session_id, limit, include_superseded))
        return "# AideMemo Session Canvas"

    def project_profile(self, *, limit=80, source_id=None, include_sessions=False):
        self.artifact_calls.append(("project_profile", limit, source_id, include_sessions))
        return "# AideMemo Project Profile"


def test_search_many_preserves_order_and_metadata() -> None:
    sdk = AideMemoMemorySDK(FakeClient())

    rows = sdk.search_many(
        [
            {"query": "redis", "vendor": "cache"},
            {"query": "postgres", "vendor": "db", "source_id": "team-b"},
        ],
        source_id="team-a",
        concurrency=2,
    )

    assert [row["query"] for row in rows] == ["redis", "postgres"]
    assert rows[0]["vendor"] == "cache"
    assert rows[0]["hits"][0]["source_id"] == "team-a"
    assert rows[1]["hits"][0]["source_id"] == "team-b"


def test_open_builds_default_client(monkeypatch) -> None:
    created = {}

    class RecordingClient(FakeClient):
        def __init__(self, **kwargs) -> None:
            super().__init__()
            created.update(kwargs)

    monkeypatch.setattr("aidememo_agent.sdk.AideMemoClient", RecordingClient)

    sdk = AideMemoMemorySDK.open(
        store_path="/tmp/wiki.sqlite",
        source_id="team-a",
        lock_retry_ms=250,
        storage_backend="libsqlite",
    )

    assert isinstance(sdk.client, RecordingClient)
    assert created == {
        "store_path": "/tmp/wiki.sqlite",
        "source_id": "team-a",
        "lock_retry_ms": 250,
        "storage_backend": "libsqlite",
    }


def test_client_passes_storage_backend_to_pyo3(monkeypatch) -> None:
    opened = {}

    class FakeAideMemo:
        def __init__(self, store_path, **kwargs) -> None:
            opened["store_path"] = store_path
            opened["kwargs"] = kwargs

    monkeypatch.setitem(sys.modules, "aidememo_python", types.SimpleNamespace(AideMemo=FakeAideMemo))

    client = AideMemoClient(store_path="/tmp/wiki.sqlite", storage_backend=" libsqlite ")

    assert client.backend == "aidememo-python"
    assert client.storage_backend == "libsqlite"
    assert opened == {"store_path": "/tmp/wiki.sqlite", "kwargs": {"backend": "libsqlite"}}


def test_cli_backend_override_is_forwarded(monkeypatch) -> None:
    captured = {}

    class Completed:
        returncode = 0
        stdout = "{}"
        stderr = ""

    def fake_run(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return Completed()

    monkeypatch.setattr("aidememo_agent.client.subprocess.run", fake_run)
    client = AideMemoClient.__new__(AideMemoClient)
    client.store_path = "/tmp/wiki.sqlite"
    client.storage_backend = "libsqlite"
    client.lock_retry_ms = 0

    assert client._cli_json(["stats"]) == {}
    assert captured["cmd"] == [
        "aidememo",
        "--backend",
        "libsqlite",
        "--store",
        "/tmp/wiki.sqlite",
        "--json",
        "stats",
    ]


def test_flatten_dedupe_group_and_coverage() -> None:
    sdk = AideMemoMemorySDK(FakeClient())
    batches = [
        {
            "query": "redis",
            "vendor": "cache",
            "hits": [
                {"fact_id": "a", "content": "A", "entity_names": ["Redis"], "fact_type": "decision"},
                {"fact_id": "a", "content": "A again", "entity_names": ["Redis"], "fact_type": "decision"},
                {"fact_id": "b", "content": "B", "entity_names": ["Worker"], "fact_type": "lesson"},
            ],
        }
    ]

    flat = sdk.flatten_hits(batches)
    deduped = sdk.dedupe_by_fact(flat)
    groups = sdk.group_by_entity(deduped)
    coverage = sdk.coverage_by(deduped, ["vendor", "fact_type"])

    assert len(flat) == 3
    assert [row["fact_id"] for row in deduped] == ["a", "b"]
    assert sorted(groups) == ["Redis", "Worker"]
    assert coverage["total"] == 2
    assert {"vendor": "cache", "fact_type": "decision", "count": 1} in coverage["groups"]
    assert {"vendor": "cache", "fact_type": "lesson", "count": 1} in coverage["groups"]


def test_search_rows_flattens_and_dedupes_default_path() -> None:
    sdk = AideMemoMemorySDK(FakeClient())

    rows = sdk.search_rows(
        [
            {"query": "redis", "vendor": "cache"},
            {"query": "redis", "vendor": "cache"},
        ],
        source_id="team-a",
    )

    assert len(rows) == 1
    assert rows[0]["query"] == "redis"
    assert rows[0]["vendor"] == "cache"
    assert rows[0]["source_id"] == "team-a"


def test_to_fact_batch_and_commit() -> None:
    client = FakeClient()
    sdk = AideMemoMemorySDK(client)

    items = sdk.to_fact_batch(
        [
            {
                "observation": "Decision: use advisory locks",
                "fact_type": "decision",
                "entities": ["BillingExport"],
            },
            {"content": "Lesson: retry races duplicate exports"},
        ],
        default_fact_type="lesson",
        default_entities=["Experiment"],
        source_id="research-alpha",
        session_id="session-1",
        tags=["scenario-n"],
    )
    ids = sdk.commit_fact_batch(items)

    assert ids == ["fact-1", "fact-2"]
    assert items[0]["fact_type"] == "decision"
    assert items[1]["fact_type"] == "lesson"
    assert items[1]["entities"] == ["Experiment"]
    assert all(item["source_id"] == "research-alpha" for item in items)
    assert all(item["session_id"] == "session-1" for item in items)
    assert client.fact_batches == [items]


def test_remember_converts_and_commits_batch() -> None:
    client = FakeClient()
    sdk = AideMemoMemorySDK(client)

    ids = sdk.remember(
        [{"content": "Decision: keep SDK first-use path short", "entities": ["Hermes"]}],
        default_fact_type="decision",
        source_id="team-a",
        tags=["ux"],
    )

    assert ids == ["fact-1"]
    assert client.fact_batches[0] == [
        {
            "content": "Decision: keep SDK first-use path short",
            "fact_type": "decision",
            "entities": ["Hermes"],
            "tags": ["ux"],
            "source_id": "team-a",
        }
    ]


def test_branch_helpers_forward_to_client() -> None:
    client = FakeClient()
    sdk = AideMemoMemorySDK(client)

    push = sdk.branch_push("candidate-b", "/tmp/shared", base="/tmp/shared/backup-01")
    merge = sdk.branch_merge("/tmp/shared", branch="candidate-b")

    assert push["branch_id"] == "candidate-b"
    assert merge["facts_inserted"] == 1
    assert client.branch_calls == [
        ("push", "candidate-b", "/tmp/shared", "/tmp/shared/backup-01"),
        ("merge", "candidate-b", "/tmp/shared"),
    ]


def test_artifact_helpers_forward_to_client() -> None:
    client = FakeClient()
    sdk = AideMemoMemorySDK(client)

    assert sdk.session_canvas("session-1", limit=12) == "# AideMemo Session Canvas"
    assert sdk.project_profile(limit=20, source_id="team-a") == "# AideMemo Project Profile"
    assert client.artifact_calls == [
        ("session_canvas", "session-1", 12, False),
        ("project_profile", 20, "team-a", False),
    ]


def test_client_artifact_methods_use_mcp_tools(monkeypatch) -> None:
    calls = []

    class Completed:
        returncode = 0
        stderr = ""

        def __init__(self, stdout: str) -> None:
            self.stdout = stdout

    def fake_run(cmd, **kwargs):
        lines = [json.loads(line) for line in kwargs["input"].splitlines() if line.strip()]
        tool_call = next(line for line in lines if line["method"] == "tools/call")
        calls.append((cmd, tool_call["params"]))
        name = tool_call["params"]["name"]
        content = "# AideMemo Session Canvas" if name == "aidememo_session_canvas" else "# AideMemo Project Profile"
        stdout = "\n".join(
            [
                json.dumps({"jsonrpc": "2.0", "id": 0, "result": {}}),
                json.dumps({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": json.dumps({"content": content}),
                            }
                        ]
                    },
                }),
            ]
        )
        return Completed(stdout)

    monkeypatch.setattr("aidememo_agent.client.subprocess.run", fake_run)
    client = AideMemoClient.__new__(AideMemoClient)
    client.store_path = "/tmp/wiki.sqlite"
    client.storage_backend = "libsqlite"
    client.lock_retry_ms = 0
    client.default_source_id = "team-a"
    client._py = None

    assert client.session_canvas("session-1", limit=7) == "# AideMemo Session Canvas"
    assert client.project_profile(limit=9) == "# AideMemo Project Profile"
    assert calls[0][0] == ["aidememo", "--backend", "libsqlite", "--store", "/tmp/wiki.sqlite", "mcp"]
    assert calls[0][1] == {
        "name": "aidememo_session_canvas",
        "arguments": {"limit": 7, "include_superseded": False, "session": "session-1"},
    }
    assert calls[1][1] == {
        "name": "aidememo_profile_export",
        "arguments": {"limit": 9, "include_sessions": False, "source_id": "team-a"},
    }


def test_pyo3_backend_preserves_session_and_context_scope() -> None:
    class FakePyBackend:
        def __init__(self) -> None:
            self.fact_add_kwargs = {}
            self.fact_add_many_items = []

        def resolve_entity(self, name):
            return f"entity-{name}"

        def fact_add(self, content, **kwargs):
            self.fact_add_kwargs = {"content": content, **kwargs}
            return "fact-one"

        def fact_add_many(self, items):
            self.fact_add_many_items = items
            return ["fact-batch"]

        def fact_list(self, **kwargs):
            return [
                {"id": "recent-a", "source_id": "team-a", "fact_type": "lesson"},
                {"id": "recent-b", "source_id": "team-b", "fact_type": "lesson"},
            ]

        def pinned_facts(self, **kwargs):
            return [
                {"id": "pinned-a", "source_id": "team-a", "fact_type": "decision"},
                {"id": "pinned-b", "source_id": "team-b", "fact_type": "decision"},
            ]

    backend = FakePyBackend()
    client = AideMemoClient.__new__(AideMemoClient)
    client.store_path = "/tmp/wiki.sqlite"
    client.lock_retry_ms = 5000
    client.default_source_id = None
    client._py = backend

    assert client.fact_add(
        "Lesson: session fact",
        entities=["Redis"],
        fact_type="lesson",
        source_id="team-a",
        session_id="session-1",
    ) == "fact-one"
    assert backend.fact_add_kwargs["session_id"] == "session-1"
    assert backend.fact_add_kwargs["source_id"] == "team-a"

    assert client.fact_add_many([
        {
            "content": "Decision: session batch",
            "entities": ["Redis"],
            "fact_type": "decision",
            "source_id": "team-a",
            "session_id": "session-1",
        }
    ]) == ["fact-batch"]
    assert backend.fact_add_many_items[0]["entity_ids"] == ["entity-Redis"]
    assert backend.fact_add_many_items[0]["session_id"] == "session-1"

    context = client.context(source_id="team-a")
    assert [row["id"] for row in context["pinned"]] == ["pinned-a"]
    assert [row["id"] for row in context["recent"]] == ["recent-a"]
    assert [row["id"] for row in context["personalisation"]] == ["recent-a"]


def test_branch_uses_pyo3_for_local_paths() -> None:
    class FakePyBackend:
        def __init__(self) -> None:
            self.calls = []

        def branch_push(self, branch, destination, *, base=None):
            self.calls.append(("push", branch, destination, base))
            return {"branch_id": branch, "records_exported": 1}

        def branch_merge(self, source, *, branch=None):
            self.calls.append(("merge", branch, source))
            return {"branch": branch, "facts_inserted": 1}

    backend = FakePyBackend()
    client = AideMemoClient.__new__(AideMemoClient)
    client.store_path = "/tmp/wiki.sqlite"
    client.storage_backend = "libsqlite"
    client.lock_retry_ms = 5000
    client.default_source_id = None
    client._py = backend

    assert client.branch_push("candidate-b", "/tmp/shared", base="/tmp/shared/backup-01") == {
        "branch_id": "candidate-b",
        "records_exported": 1,
    }
    assert client.branch_merge("/tmp/shared", branch="candidate-b") == {
        "branch": "candidate-b",
        "facts_inserted": 1,
    }
    assert backend.calls == [
        ("push", "candidate-b", "/tmp/shared", "/tmp/shared/backup-01"),
        ("merge", "candidate-b", "/tmp/shared"),
    ]


def test_branch_s3_uses_cli_even_when_pyo3_exists(monkeypatch) -> None:
    captured = []

    class FakePyBackend:
        pass

    class Completed:
        returncode = 0
        stderr = ""

        def __init__(self, stdout):
            self.stdout = stdout

    def fake_run(cmd, **kwargs):
        captured.append(cmd)
        if "push" in cmd:
            return Completed('{"branch_id":"candidate-b","records_exported":1}')
        return Completed('{"branch":"candidate-b","facts_inserted":1}')

    monkeypatch.setattr("aidememo_agent.client.subprocess.run", fake_run)
    client = AideMemoClient.__new__(AideMemoClient)
    client.store_path = "/tmp/wiki.sqlite"
    client.storage_backend = "libsqlite"
    client.lock_retry_ms = 0
    client.default_source_id = None
    client._py = FakePyBackend()

    assert client.branch_push("candidate-b", "s3://bucket/shared", base="s3://bucket/shared/backup-01")[
        "records_exported"
    ] == 1
    assert client.branch_merge("s3://bucket/shared", branch="candidate-b")["facts_inserted"] == 1
    assert captured == [
        [
            "aidememo",
            "--backend",
            "libsqlite",
            "--store",
            "/tmp/wiki.sqlite",
            "--json",
            "branch",
            "push",
            "--branch",
            "candidate-b",
            "--base",
            "s3://bucket/shared/backup-01",
            "s3://bucket/shared",
        ],
        [
            "aidememo",
            "--backend",
            "libsqlite",
            "--store",
            "/tmp/wiki.sqlite",
            "--json",
            "branch",
            "merge",
            "--branch",
            "candidate-b",
            "s3://bucket/shared",
        ],
    ]


def test_query_and_aggregate_many_forward_source_scope() -> None:
    sdk = AideMemoMemorySDK(FakeClient())

    contexts = sdk.query_many(["redis", {"topic": "billing", "source_id": "team-b"}], source_id="team-a")
    aggregates = sdk.aggregate_many(["redis decisions"], op="count", source_id="team-a")

    assert contexts[0]["context"]["source_id"] == "team-a"
    assert contexts[1]["context"]["source_id"] == "team-b"
    assert aggregates[0]["result"]["source_id"] == "team-a"
