"""End-to-end smoke test for wg-python.

Run after `maturin develop` (or `pip install` of the wheel).
Exercises every public method against a temp DB so we catch breakage early.
"""

import os
import shutil
import tempfile

import wg_python as wg


def main() -> None:
    tmp = tempfile.mkdtemp(prefix="wg-py-smoke-")
    db = os.path.join(tmp, "test.redb")
    try:
        g = wg.WikiGraph(db)

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

        # Facts
        fid = g.fact_add(
            "Redis Sentinel provides high availability",
            entity_ids=[eid_redis],
            fact_type="decision",
            tags=["ha"],
            confidence=0.9,
        )
        fact = g.fact_get(fid)
        assert fact["content"].startswith("Redis Sentinel")

        facts = g.fact_list(entity="Redis", limit=10)
        assert len(facts) == 1

        # Relations
        g.relation_add("Redis", "Postgres", "alternative_to")
        rels = g.relations_get("Redis", direction="forward")
        assert len(rels) == 1

        # Search (BM25 — no semantic model loaded for the smoke test, but the
        # call path should still succeed and return results from BM25 only).
        try:
            hits = g.search("high availability", limit=5)
            print(f"search hits: {len(hits)}")
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

        # Query (unified) — relies on hybrid search; tolerate failure on that
        # but verify the call dispatches.
        try:
            ctx = g.query("Redis", limit=3, depth=1, recent_limit=3)
            assert ctx["topic"] == "Redis"
            assert ctx["entity"]["name"] == "Redis"
            print(f"query keys: {list(ctx.keys())}")
        except RuntimeError as e:
            print(f"query skipped: {e}")

        # Cleanup writes
        g.fact_delete(fid)
        g.relation_remove("Redis", "Postgres", "alternative_to")
        g.entity_delete("Postgres")

        print("OK: wg-python smoke test passed")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
