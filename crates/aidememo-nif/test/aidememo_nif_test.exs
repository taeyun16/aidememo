defmodule AideMemoNifTest do
  use ExUnit.Case

  setup do
    tmp = System.tmp_dir!() |> Path.join("aidememo-nif-#{System.unique_integer([:positive])}")
    File.mkdir_p!(tmp)
    db = Path.join(tmp, "test.redb")
    on_exit(fn -> File.rm_rf!(tmp) end)
    {:ok, db: db}
  end

  test "full smoke: entity + fact + relation + traverse + query", %{db: db} do
    g = AideMemoNif.open!(db)
    assert is_reference(g)

    # Entity CRUD
    eid_redis =
      AideMemoNif.entity_add(g, "Redis",
        entity_type: "technology",
        tags: ["cache", "infra"],
        aliases: ["redis-server"]
      )

    _eid_postgres = AideMemoNif.entity_add(g, "Postgres", entity_type: "technology")

    assert AideMemoNif.resolve_entity(g, "Redis") == eid_redis
    assert AideMemoNif.resolve_entity(g, "redis-server") == eid_redis

    e = AideMemoNif.entity_get(g, "Redis")
    assert e["name"] == "Redis"
    assert "cache" in e["tags"]

    ents = AideMemoNif.entity_list(g, limit: 10)
    assert length(ents) == 2

    # Facts
    fid =
      AideMemoNif.fact_add(g, "Redis Sentinel provides high availability",
        entity_ids: [eid_redis],
        fact_type: "decision",
        tags: ["ha"],
        confidence: 0.9
      )

    fact = AideMemoNif.fact_get(g, fid)
    assert String.starts_with?(fact["content"], "Redis Sentinel")

    facts = AideMemoNif.fact_list(g, entity: "Redis", limit: 10)
    assert length(facts) == 1

    # Batch insert — single redb write txn for the whole list.
    eid_postgres = AideMemoNif.resolve_entity(g, "Postgres")

    many_ids =
      AideMemoNif.fact_add_many(g, [
        %{
          content: "Redis Cluster shards by hash slot",
          entity_ids: [eid_redis],
          fact_type: "pattern"
        },
        %{
          content: "Redis 7 introduces Functions and ACL improvements",
          entity_ids: [eid_redis],
          fact_type: "note",
          confidence: 0.85
        },
        %{
          content: "Postgres logical replication is the default",
          entity_ids: [eid_postgres],
          fact_type: "convention"
        }
      ])

    assert length(many_ids) == 3
    assert Enum.all?(many_ids, &is_binary/1)

    for id <- many_ids do
      rec = AideMemoNif.fact_get(g, id)
      assert rec["id"] == id
    end

    # Relations
    :ok = AideMemoNif.relation_add(g, "Redis", "Postgres", "alternative_to")
    rels = AideMemoNif.relations_get(g, "Redis", direction: "forward")
    assert length(rels) == 1

    # Search (BM25 + semantic)
    hits = AideMemoNif.search(g, "high availability", limit: 5)
    assert is_list(hits)

    # Graph
    traverse = AideMemoNif.traverse(g, "Redis", depth: 1, direction: "both")
    assert is_list(traverse["entities"])

    path = AideMemoNif.path_find(g, "Redis", "Postgres")
    assert is_list(path) and length(path) >= 1

    # Lint / stats
    issues = AideMemoNif.lint(g)
    assert is_list(issues)
    stats = AideMemoNif.stats(g)
    assert stats["entity_count"] == 2

    # Query (unified)
    ctx = AideMemoNif.query(g, "Redis", limit: 3, depth: 1, recent_limit: 3)
    assert ctx["topic"] == "Redis"
    assert ctx["entity"]["name"] == "Redis"
    assert Map.has_key?(ctx, "search")
    assert Map.has_key?(ctx, "related")
    assert Map.has_key?(ctx, "recent_facts")

    # Validity windows
    new_fid =
      AideMemoNif.fact_add(g, "Redis Sentinel + Cluster supersedes Sentinel-only HA",
        entity_ids: [eid_redis],
        fact_type: "decision"
      )

    :ok = AideMemoNif.fact_supersede(g, fid, new_fid)
    old = AideMemoNif.fact_get(g, fid)
    assert old["superseded_at"] != nil
    assert old["superseded_by"] == new_fid

    all_facts = AideMemoNif.fact_list(g, entity: "Redis")
    current_facts = AideMemoNif.fact_list(g, entity: "Redis", current_only: true)
    assert length(current_facts) == length(all_facts) - 1

    # Cleanup writes
    :ok = AideMemoNif.fact_delete(g, fid)
    :ok = AideMemoNif.fact_delete(g, new_fid)
    :ok = AideMemoNif.relation_remove(g, "Redis", "Postgres", "alternative_to")
    :ok = AideMemoNif.entity_delete(g, "Postgres")
  end

  test "version is exposed" do
    assert is_binary(AideMemoNif.version())
  end
end
