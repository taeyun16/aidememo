defmodule AideMemoNifTest do
  use ExUnit.Case

  setup do
    tmp = System.tmp_dir!() |> Path.join("aidememo-nif-#{System.unique_integer([:positive])}")
    File.mkdir_p!(tmp)
    db = Path.join(tmp, "test.sqlite")
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

    # Batch insert — single backend transaction for the whole list.
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

  test "branch logs push and merge through open handles", %{db: db} do
    root = Path.dirname(db)
    shared = Path.join(root, "shared-branches")

    candidate_a = AideMemoNif.open!(Path.join(root, "candidate-a.sqlite"))
    entity_a = AideMemoNif.entity_add(candidate_a, "BranchExperiment", entity_type: "concept")

    _fact_a =
      AideMemoNif.fact_add(candidate_a, "Candidate A keeps the red branch result",
        entity_ids: [entity_a],
        fact_type: "lesson"
      )

    push_a = AideMemoNif.branch_push(candidate_a, "candidate-a", shared)
    assert push_a["branch_id"] == "candidate-a"
    assert push_a["records_exported"] >= 2
    assert push_a["export_mode"] == "full"

    candidate_b = AideMemoNif.open!(Path.join(root, "candidate-b.sqlite"))
    entity_b = AideMemoNif.entity_add(candidate_b, "BranchExperiment", entity_type: "concept")

    _fact_b =
      AideMemoNif.fact_add(candidate_b, "Candidate B keeps the blue branch result",
        entity_ids: [entity_b],
        fact_type: "lesson"
      )

    push_b = AideMemoNif.branch_push(candidate_b, "candidate-b", shared)
    assert push_b["branch_id"] == "candidate-b"
    assert push_b["records_exported"] >= 2

    target = AideMemoNif.open!(Path.join(root, "target.sqlite"))
    merge_b = AideMemoNif.branch_merge(target, shared, branch: "candidate-b")
    assert merge_b["branch"] == "candidate-b"
    assert merge_b["segments_merged"] == 1
    assert merge_b["facts_inserted"] == 1

    target_facts = AideMemoNif.fact_list(target, entity: "BranchExperiment", limit: 10)
    contents = Enum.map(target_facts, & &1["content"])
    assert "Candidate B keeps the blue branch result" in contents
    refute "Candidate A keeps the red branch result" in contents

    repeat = AideMemoNif.branch_merge(target, shared, branch: "candidate-b")
    assert repeat["segments_merged"] == 1
    assert repeat["facts_inserted"] == 0

    all_target = AideMemoNif.open!(Path.join(root, "all-target.sqlite"))
    merge_all = AideMemoNif.branch_merge(all_target, shared)
    assert merge_all["branch"] == nil
    assert merge_all["segments_merged"] == 2
    assert merge_all["facts_inserted"] == 2

    assert_raise ArgumentError, ~r/S3 branch logs/, fn ->
      AideMemoNif.branch_push(candidate_a, "candidate-a", "s3://bucket/prefix")
    end

    assert_raise ArgumentError, ~r/S3 branch logs/, fn ->
      AideMemoNif.branch_merge(target, "s3://bucket/prefix")
    end
  end

  test "default and empty backend open the compiled default store", %{db: db} do
    g = AideMemoNif.open!(db)
    assert is_reference(g)
    assert %{"fact_count" => 0} = AideMemoNif.stats(g)
    assert_backend_file(db, "sqlite")

    empty_backend_path = Path.rootname(db) <> ".empty-backend"
    empty = AideMemoNif.open!(empty_backend_path, backend: "")
    assert is_reference(empty)
    assert %{"fact_count" => 0} = AideMemoNif.stats(empty)
    assert_backend_file(empty_backend_path, "sqlite")
  end

  defp assert_backend_file(path, backend) when backend in ["sqlite", "libsqlite"] do
    assert File.exists?(path)
    assert File.read!(path) |> binary_part(0, 16) == "SQLite format 3\0"
  end

  defp assert_backend_file(path, "redb") do
    assert File.exists?(path)
    refute File.read!(path) |> binary_part(0, 16) == "SQLite format 3\0"
  end

  defp assert_backend_opens(db, backend) do
    store_path = Path.rootname(db) <> ".#{backend}"
    g = AideMemoNif.open!(store_path, backend: backend)
    assert is_reference(g)

    label = backend |> String.capitalize()
    eid = AideMemoNif.entity_add(g, "Elixir#{label}", entity_type: "technology")

    fid =
      AideMemoNif.fact_add(g, "Elixir NIF opened a #{backend} backend",
        entity_ids: [eid],
        fact_type: "note"
      )

    assert is_binary(fid)
    stats = AideMemoNif.stats(g)
    assert stats["entity_count"] == 1
    assert stats["fact_count"] == 1
    assert_backend_file(store_path, backend)
  end

  test "sqlite backend opens when cargo feature is enabled", %{db: db} do
    assert_backend_opens(db, "sqlite")
  end

  test "libsqlite backend alias opens SQLite when cargo feature is enabled", %{db: db} do
    assert_backend_opens(db, "libsqlite")
  end

  test "redb backend opens when cargo feature is enabled", %{db: db} do
    features = System.get_env("AIDEMEMO_NIF_CARGO_FEATURES", "")

    if String.contains?(features, "redb") do
      assert_backend_opens(db, "redb")
    end
  end
end
