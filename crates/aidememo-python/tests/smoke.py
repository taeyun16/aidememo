"""End-to-end smoke test for aidememo-python.

Run after `maturin develop` (or `pip install` of the wheel).
Exercises every public method against a temp DB so we catch breakage early.
"""

import os
import shutil
import tempfile

import aidememo_python as aidememo


SQLITE_HEADER = b"SQLite format 3\x00"


def smoke_backend() -> str:
    backend = os.environ.get("AIDEMEMO_PYTHON_SMOKE_BACKEND", "sqlite").strip().lower()
    if backend == "":
        return "sqlite"
    if backend not in {"sqlite", "libsqlite", "redb"}:
        raise AssertionError(f"unsupported AIDEMEMO_PYTHON_SMOKE_BACKEND={backend!r}")
    return backend


def read_header(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read(16)


def assert_backend_file(path: str, backend: str) -> None:
    header = read_header(path)
    if backend in {"sqlite", "libsqlite"}:
        assert header == SQLITE_HEADER, header
    elif backend == "redb":
        assert header != SQLITE_HEADER, "redb backend produced a SQLite store file"


def assert_default_and_empty_backend_match(tmp: str, explicit_backend: str) -> None:
    expected = os.environ.get("AIDEMEMO_PYTHON_SMOKE_EXPECT_DEFAULT_BACKEND", "").strip().lower()
    if not expected and explicit_backend in {"sqlite", "libsqlite"}:
        expected = "sqlite"
    if expected and expected not in {"sqlite", "redb"}:
        raise AssertionError(
            f"unsupported AIDEMEMO_PYTHON_SMOKE_EXPECT_DEFAULT_BACKEND={expected!r}"
        )

    default_db = os.path.join(tmp, "default-store")
    empty_db = os.path.join(tmp, "empty-backend-store")

    aidememo.AideMemo(default_db).stats()
    aidememo.AideMemo(empty_db, backend="").stats()

    default_header = read_header(default_db)
    empty_header = read_header(empty_db)
    assert empty_header == default_header, "empty backend should inherit compiled default"
    if expected:
        assert_backend_file(default_db, expected)
        assert_backend_file(empty_db, expected)


def main() -> None:
    tmp = tempfile.mkdtemp(prefix="aidememo-py-smoke-")
    backend = smoke_backend()
    db = os.path.join(tmp, f"test.{backend}")
    try:
        assert_default_and_empty_backend_match(tmp, backend)
        g = aidememo.AideMemo(db, backend=backend)

        # Entity CRUD
        eid_redis = g.entity_add(
            "Redis",
            entity_type="technology",
            tags=["cache", "infra"],
            aliases=["redis-server"],
        )
        eid_postgres = g.entity_add("Postgres", entity_type="technology")
        assert g.resolve_entity("Redis") == eid_redis
        assert g.resolve_entity("redis-server") == eid_redis  # alias

        e = g.entity_get("Redis")
        assert e["name"] == "Redis"
        assert "cache" in e["tags"]

        ents = g.entity_list(limit=10)
        assert len(ents) == 2
        assert {x["name"] for x in ents} == {"Redis", "Postgres"}

        try:
            g.entity_get("Rdis")
            raise AssertionError("entity_get should raise AideMemoNotFoundError")
        except aidememo.AideMemoNotFoundError as e:
            assert isinstance(e, aidememo.AideMemoError)
            assert "[entity_not_found]" in str(e)
            assert "Rdis" in str(e)

        # Facts
        fid = g.fact_add(
            "Redis Sentinel provides high availability",
            entity_ids=[eid_redis],
            fact_type="decision",
            tags=["ha"],
            confidence=0.9,
            source_id="team-a",
        )
        fact = g.fact_get(fid)
        assert fact["content"].startswith("Redis Sentinel")
        assert fact["source_id"] == "team-a"
        assert_backend_file(db, backend)

        g.fact_pin(fid, True)
        pinned = g.pinned_facts(limit=5)
        assert any(f["id"] == fid for f in pinned), "pinned_facts should surface pinned fact"
        g.fact_pin(fid, False)
        assert all(f["id"] != fid for f in g.pinned_facts(limit=5))

        facts = g.fact_list(entity="Redis", limit=10, source_id="team-a")
        assert len(facts) == 1
        assert all(f["source_id"] == "team-a" for f in facts)

        # Batch insert — single backend transaction for the whole list.
        many_ids = g.fact_add_many([
            {"content": "Redis worker timeout lesson: DNS resolution caused queue stalls",
             "entity_ids": [eid_redis], "fact_type": "lesson", "source_id": "team-a"},
            {"content": "Redis timeout error: missing DNS metrics hid resolver failures",
             "entity_ids": [eid_redis], "fact_type": "error", "confidence": 0.85,
             "source_id": "team-a"},
            {"content": "Postgres logical replication is the default",
             "entity_ids": [eid_postgres], "fact_type": "convention", "source_id": "team-b"},
        ])
        assert len(many_ids) == 3
        assert all(isinstance(x, str) for x in many_ids)
        # The new facts are findable.
        for fid_ in many_ids:
            rec = g.fact_get(fid_)
            assert rec["id"] == fid_

        # Relations
        g.relation_add("Redis", "Postgres", "alternative_to")
        rels = g.relations_get("Redis", direction="forward")
        assert len(rels) == 1

        # Search (BM25 — no semantic model loaded for the smoke test, but the
        # call path should still succeed and return results from BM25 only).
        try:
            hits = g.search("high availability", limit=5, source_id="team-a")
            print(f"search hits: {len(hits)}")
            assert all(h["source_id"] == "team-a" for h in hits)
        except RuntimeError as e:
            # If semantic model isn't downloaded, hybrid search may fail — that's
            # an environment issue, not a binding issue. Don't fail the test.
            print(f"search skipped: {e}")

        # Graph
        traverse = g.traverse("Redis", depth=1, direction="both")
        assert "entities" in traverse
        path = g.path_find("Redis", "Postgres")
        assert path is not None and len(path) >= 1

        # Lint / stats
        issues = g.lint()
        stats = g.stats()
        print(f"stats: {stats}")
        print(f"lint issues: {len(issues)}")

        # Branch logs: native binding should be able to push/merge from an
        # already-open handle without reopening the same store. This matters
        # for SDK/plugin callers and for redb builds with exclusive handles.
        branch_dir = os.path.join(tmp, "branches")
        branch_clone_db = os.path.join(tmp, f"branch-clone.{backend}")
        branch_clone = aidememo.AideMemo(branch_clone_db, backend=backend)
        branch_push = g.branch_push("python-smoke", branch_dir)
        assert branch_push["branch_id"] == "python-smoke"
        assert branch_push["records_exported"] >= stats["fact_count"]
        branch_merge = branch_clone.branch_merge(branch_dir, branch="python-smoke")
        assert branch_merge["segments_merged"] == 1
        assert branch_merge["facts_inserted"] == stats["fact_count"]
        assert branch_clone.stats()["fact_count"] == stats["fact_count"]
        branch_merge_again = branch_clone.branch_merge(branch_dir, branch="python-smoke")
        assert branch_merge_again["facts_inserted"] == 0

        # Query (unified) — relies on hybrid search; tolerate failure on that
        # but verify the call dispatches.
        try:
            ctx = g.query("Redis", limit=3, depth=1, recent_limit=3, source_id="team-a")
            assert ctx["topic"] == "Redis"
            assert ctx["entity"]["name"] == "Redis"
            assert all(h["source_id"] == "team-a" for h in ctx["search"])
            print(f"query keys: {list(ctx.keys())}")
        except RuntimeError as e:
            print(f"query skipped: {e}")

        # Workflow start — sparse ticket entrypoint for SDK-style callers.
        pack = g.workflow_start(
            "Fix Redis timeout in worker",
            body="Worker jobs time out against Redis with sparse issue details.",
            source="github:org/app#123",
            source_id="team-a",
            limit=5,
            depth=1,
            recent_limit=3,
            bm25_only=True,
        )
        assert pack["session_id"].startswith("session-")
        assert pack["source_id"] == "team-a"
        assert pack["ticket_fact_id"]
        assert pack["context"]["topic"].startswith("Fix Redis timeout")
        assert all(h["source_id"] == "team-a" for h in pack["context"]["search"])
        assert pack["prior_lessons"], "workflow_start should surface scoped lessons"
        assert pack["prior_errors"], "workflow_start should surface scoped errors"
        assert pack["relevant_decisions"], "workflow_start should surface scoped decisions"

        session_fact_id = g.fact_add(
            "Lesson: Redis workflow follow-up facts should attach to the tracked session",
            entity_ids=[eid_redis],
            fact_type="lesson",
            source_id="team-a",
            session_id=pack["session_id"],
        )
        session_many_ids = g.fact_add_many(
            [
                {
                    "content": "Decision: Redis workflow batches keep the same session thread",
                    "entity_ids": [eid_redis],
                    "fact_type": "decision",
                    "source_id": "team-a",
                }
            ],
            session_id=pack["session_id"],
        )
        session_facts = g.fact_list(entity=pack["session_id"], limit=10)
        session_contents = {f["content"] for f in session_facts}
        assert g.fact_get(session_fact_id)["content"] in session_contents
        assert g.fact_get(session_many_ids[0])["content"] in session_contents

        # Validity windows: supersede the first fact with a new one and verify
        # current_only filtering hides it.
        new_fid = g.fact_add(
            "Redis Sentinel + Cluster supersedes Sentinel-only HA",
            entity_ids=[eid_redis],
            fact_type="decision",
        )
        g.fact_supersede(fid, new_fid)
        old = g.fact_get(fid)
        assert old["superseded_at"] is not None, "fact_supersede should set superseded_at"
        assert old["superseded_by"] == new_fid

        all_facts = g.fact_list(entity="Redis")
        current_facts = g.fact_list(entity="Redis", current_only=True)
        assert len(current_facts) == len(all_facts) - 1, "current_only should hide superseded"

        # Cleanup writes
        g.fact_delete(fid)
        g.fact_delete(new_fid)
        g.relation_remove("Redis", "Postgres", "alternative_to")
        g.entity_delete("Postgres")

        print(f"OK: aidememo-python smoke test passed ({backend})")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
