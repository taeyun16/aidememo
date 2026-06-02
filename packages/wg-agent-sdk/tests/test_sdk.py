from __future__ import annotations

from wg_agent.sdk import WgMemorySDK


class FakeClient:
    def __init__(self) -> None:
        self.fact_batches = []

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


def test_search_many_preserves_order_and_metadata() -> None:
    sdk = WgMemorySDK(FakeClient())

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

    monkeypatch.setattr("wg_agent.sdk.WgClient", RecordingClient)

    sdk = WgMemorySDK.open(store_path="/tmp/wiki.redb", source_id="team-a", lock_retry_ms=250)

    assert isinstance(sdk.client, RecordingClient)
    assert created == {"store_path": "/tmp/wiki.redb", "source_id": "team-a", "lock_retry_ms": 250}


def test_flatten_dedupe_group_and_coverage() -> None:
    sdk = WgMemorySDK(FakeClient())
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
    sdk = WgMemorySDK(FakeClient())

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
    sdk = WgMemorySDK(client)

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
    sdk = WgMemorySDK(client)

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


def test_query_and_aggregate_many_forward_source_scope() -> None:
    sdk = WgMemorySDK(FakeClient())

    contexts = sdk.query_many(["redis", {"topic": "billing", "source_id": "team-b"}], source_id="team-a")
    aggregates = sdk.aggregate_many(["redis decisions"], op="count", source_id="team-a")

    assert contexts[0]["context"]["source_id"] == "team-a"
    assert contexts[1]["context"]["source_id"] == "team-b"
    assert aggregates[0]["result"]["source_id"] == "team-a"
