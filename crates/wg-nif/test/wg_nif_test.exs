defmodule WgNifTest do
  use ExUnit.Case

  setup do
    tmp = System.tmp_dir!() |> Path.join("wg-nif-#{System.unique_integer([:positive])}")
    File.mkdir_p!(tmp)
    db = Path.join(tmp, "test.redb")
    on_exit(fn -> File.rm_rf!(tmp) end)
    {:ok, db: db}
  end

  test "full smoke: entity + fact + relation + traverse + query", %{db: db} do
    g = WgNif.open!(db)
    assert is_reference(g)

    # Entity CRUD
    eid_redis =
      WgNif.entity_add(g, "Redis",
        entity_type: "technology",
        tags: ["cache", "infra"],
        aliases: ["redis-server"]
      )

    _eid_postgres = WgNif.entity_add(g, "Postgres", entity_type: "technology")

    assert WgNif.resolve_entity(g, "Redis") == eid_redis
    assert WgNif.resolve_entity(g, "redis-server") == eid_redis

    e = WgNif.entity_get(g, "Redis")
    assert e["name"] == "Redis"
    assert "cache" in e["tags"]

    ents = WgNif.entity_list(g, limit: 10)
    assert length(ents) == 2

    # Facts
    fid =
      WgNif.fact_add(g, "Redis Sentinel provides high availability",
        entity_ids: [eid_redis],
        fact_type: "decision",
        tags: ["ha"],
        confidence: 0.9
      )

    fact = WgNif.fact_get(g, fid)
    assert String.starts_with?(fact["content"], "Redis Sentinel")

    facts = WgNif.fact_list(g, entity: "Redis", limit: 10)
    assert length(facts) == 1

    # Relations
    :ok = WgNif.relation_add(g, "Redis", "Postgres", "alternative_to")
    rels = WgNif.relations_get(g, "Redis", direction: "forward")
    assert length(rels) == 1

    # Search (BM25 + semantic)
    hits = WgNif.search(g, "high availability", limit: 5)
    assert is_list(hits)

    # Graph
    traverse = WgNif.traverse(g, "Redis", depth: 1, direction: "both")
    assert is_list(traverse["entities"])

    path = WgNif.path_find(g, "Redis", "Postgres")
    assert is_list(path) and length(path) >= 1

    # Lint / stats
    issues = WgNif.lint(g)
    assert is_list(issues)
    stats = WgNif.stats(g)
    assert stats["entity_count"] == 2

    # Query (unified)
    ctx = WgNif.query(g, "Redis", limit: 3, depth: 1, recent_limit: 3)
    assert ctx["topic"] == "Redis"
    assert ctx["entity"]["name"] == "Redis"
    assert Map.has_key?(ctx, "search")
    assert Map.has_key?(ctx, "related")
    assert Map.has_key?(ctx, "recent_facts")

    # Cleanup writes
    :ok = WgNif.fact_delete(g, fid)
    :ok = WgNif.relation_remove(g, "Redis", "Postgres", "alternative_to")
    :ok = WgNif.entity_delete(g, "Postgres")
  end

  test "version is exposed" do
    assert is_binary(WgNif.version())
  end
end
