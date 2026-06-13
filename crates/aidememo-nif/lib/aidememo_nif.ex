defmodule AideMemoNif do
  @moduledoc """
  Elixir bindings for AideMemo (`aidememo`) — a local knowledge-graph wiki backed
  by redb by default, with an experimental SQLite backend when the NIF is built
  with the `sqlite` Cargo feature.

  ## Quick start

      g = AideMemoNif.open!("./_meta/wiki.redb")

      ctx = AideMemoNif.query(g, "Redis")          # unified context fetch
      hits = AideMemoNif.search(g, "high availability", limit: 10)
      eid  = AideMemoNif.entity_add(g, "Redis", entity_type: "technology")
      AideMemoNif.fact_add(g, "Redis Sentinel provides HA", entity_ids: [eid])

  Read methods that return complex shapes (search, query, traverse, …)
  decode JSON automatically; you receive plain Elixir maps and lists.
  """

  alias AideMemoNif.Native

  @doc "Open a wiki store. Returns a NIF resource handle."
  def open!(path, opts \\ []) do
    backend = Keyword.get(opts, :backend, "")

    result =
      if backend in ["", nil] do
        Native.open(path)
      else
        Native.open_with_backend(path, to_string(backend))
      end

    case result do
      ref when is_reference(ref) -> ref
      handle -> handle
    end
  end

  def version, do: Native.version()

  # === Search ==============================================================

  def search(handle, query, opts \\ []) do
    limit = Keyword.get(opts, :limit, 10)
    current_only = Keyword.get(opts, :current_only, false)
    handle |> Native.search(query, limit, current_only) |> Jason.decode!()
  end

  @doc """
  Unified context fetch for `topic`. Returns a map with keys
  `["topic", "entity", "search", "related", "recent_facts"]`.
  """
  def query(handle, topic, opts \\ []) do
    limit = Keyword.get(opts, :limit, 10)
    depth = Keyword.get(opts, :depth, 2)
    recent_limit = Keyword.get(opts, :recent_limit, 10)
    current_only = Keyword.get(opts, :current_only, false)
    mode = Keyword.get(opts, :mode, "hybrid")

    handle
    |> Native.query(topic, limit, depth, recent_limit, current_only, mode)
    |> Jason.decode!()
  end

  # === Graph ===============================================================

  def traverse(handle, entity, opts \\ []) do
    depth = Keyword.get(opts, :depth, 2)
    direction = Keyword.get(opts, :direction, "both")
    handle |> Native.traverse(entity, depth, direction) |> Jason.decode!()
  end

  def path_find(handle, from, to) do
    handle |> Native.path_find(from, to) |> Jason.decode!()
  end

  # === Entity CRUD =========================================================

  def entity_add(handle, name, opts \\ []) do
    Native.entity_add(
      handle,
      name,
      Keyword.get(opts, :entity_type, ""),
      Keyword.get(opts, :tags, []),
      Keyword.get(opts, :aliases, []),
      Keyword.get(opts, :source_page, "")
    )
  end

  def entity_get(handle, name), do: Native.entity_get(handle, name) |> Jason.decode!()

  def entity_list(handle, opts \\ []) do
    limit = Keyword.get(opts, :limit, 0)
    type = Keyword.get(opts, :entity_type, "")
    handle |> Native.entity_list(limit, type) |> Jason.decode!()
  end

  def entity_delete(handle, name), do: Native.entity_delete(handle, name)

  def entity_describe(handle, name, summary),
    do: Native.entity_describe(handle, name, summary)

  def resolve_entity(handle, name), do: Native.resolve_entity(handle, name)

  # === Fact CRUD ===========================================================

  def fact_add(handle, content, opts \\ []) do
    Native.fact_add(
      handle,
      content,
      Keyword.get(opts, :entity_ids, []),
      Keyword.get(opts, :fact_type, ""),
      Keyword.get(opts, :tags, []),
      Keyword.get(opts, :source, ""),
      :erlang.float(Keyword.get(opts, :confidence, 0.0))
    )
  end

  @doc """
  Insert N facts in one redb write transaction.

  `items` is a list of maps with the same keys as `fact_add/3`'s opts
  (`content`, `entity_ids`, `fact_type`, `tags`, `source`,
  `confidence`); each map is normalized to the `FactAddManyItem`
  shape the NIF expects, with sane defaults so callers can omit
  fields that don't apply.

      AideMemoNif.fact_add_many(g, [
        %{content: "Redis 7 introduces Functions"},
        %{content: "Postgres uses logical replication", fact_type: "convention"},
      ])

  Returns the new fact ULIDs in input order. All-or-nothing.
  """
  def fact_add_many(handle, items) do
    Native.fact_add_many(
      handle,
      Enum.map(items, fn item ->
        %{
          content: Map.fetch!(item, :content),
          entity_ids: Map.get(item, :entity_ids, []),
          fact_type: Map.get(item, :fact_type, ""),
          tags: Map.get(item, :tags, []),
          source: Map.get(item, :source, ""),
          confidence: :erlang.float(Map.get(item, :confidence, 0.0))
        }
      end)
    )
  end

  def fact_get(handle, fact_id), do: Native.fact_get(handle, fact_id) |> Jason.decode!()

  def fact_list(handle, opts \\ []) do
    entity = Keyword.get(opts, :entity, "")
    type = Keyword.get(opts, :fact_type, "")
    limit = Keyword.get(opts, :limit, 0)
    current_only = Keyword.get(opts, :current_only, false)
    handle |> Native.fact_list(entity, type, limit, current_only) |> Jason.decode!()
  end

  def fact_delete(handle, fact_id), do: Native.fact_delete(handle, fact_id)

  def fact_supersede(handle, old_id, new_id),
    do: Native.fact_supersede(handle, old_id, new_id)

  # === Relations ===========================================================

  def relation_add(handle, source, target, rel_type),
    do: Native.relation_add(handle, source, target, rel_type)

  def relation_remove(handle, source, target, rel_type),
    do: Native.relation_remove(handle, source, target, rel_type)

  def relations_get(handle, entity, opts \\ []) do
    direction = Keyword.get(opts, :direction, "both")
    handle |> Native.relations_get(entity, direction) |> Jason.decode!()
  end

  # === Ingest / Lint / Stats ==============================================

  def ingest(handle, wiki_root, opts \\ []) do
    incremental = Keyword.get(opts, :incremental, false)
    handle |> Native.ingest(wiki_root, incremental) |> Jason.decode!()
  end

  def lint(handle), do: handle |> Native.lint() |> Jason.decode!()
  def stats(handle), do: handle |> Native.stats() |> Jason.decode!()
end
