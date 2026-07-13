defmodule AideMemoNif do
  @moduledoc """
  Elixir bindings for AideMemo (`aidememo`) — a local knowledge-graph wiki backed
  by SQLite by default, with an optional redb backend when the NIF is built with
  the `redb` Cargo feature.

  ## Quick start

      g = AideMemoNif.open!("./_meta/wiki.sqlite")

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
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.search(handle, query, limit, current_only)
      else
        Native.search_scoped(handle, query, limit, current_only, to_string(source_id))
      end

    Jason.decode!(result)
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
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.query(handle, topic, limit, depth, recent_limit, current_only, mode)
      else
        Native.query_scoped(
          handle,
          topic,
          limit,
          depth,
          recent_limit,
          current_only,
          mode,
          to_string(source_id)
        )
      end

    Jason.decode!(result)
  end

  # === Graph ===============================================================

  def traverse(handle, entity, opts \\ []) do
    depth = Keyword.get(opts, :depth, 2)
    direction = Keyword.get(opts, :direction, "both")
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.traverse(handle, entity, depth, direction)
      else
        Native.traverse_scoped(handle, entity, depth, direction, to_string(source_id))
      end

    Jason.decode!(result)
  end

  def path_find(handle, from, to, opts \\ []) do
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.path_find(handle, from, to)
      else
        Native.path_find_scoped(handle, from, to, to_string(source_id))
      end

    Jason.decode!(result)
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

  def entity_get(handle, name, opts \\ []) do
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.entity_get(handle, name)
      else
        Native.entity_get_scoped(handle, name, to_string(source_id))
      end

    Jason.decode!(result)
  end

  def entity_list(handle, opts \\ []) do
    limit = Keyword.get(opts, :limit, 0)
    type = Keyword.get(opts, :entity_type, "")
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.entity_list(handle, limit, type)
      else
        Native.entity_list_scoped(handle, limit, type, to_string(source_id))
      end

    Jason.decode!(result)
  end

  def entity_delete(handle, name), do: Native.entity_delete(handle, name)

  def entity_describe(handle, name, summary),
    do: Native.entity_describe(handle, name, summary)

  def resolve_entity(handle, name), do: Native.resolve_entity(handle, name)

  # === Fact CRUD ===========================================================

  def fact_add(handle, content, opts \\ []) do
    source_id = Keyword.get(opts, :source_id, "")
    actor_id = Keyword.get(opts, :actor_id, "")

    args = [
      handle,
      content,
      Keyword.get(opts, :entity_ids, []),
      Keyword.get(opts, :fact_type, ""),
      Keyword.get(opts, :tags, []),
      Keyword.get(opts, :source, ""),
      :erlang.float(Keyword.get(opts, :confidence, 0.0))
    ]

    if source_id in ["", nil] and actor_id in ["", nil] do
      apply(Native, :fact_add, args)
    else
      apply(Native, :fact_add_scoped, args ++ [maybe_string(source_id), maybe_string(actor_id)])
    end
  end

  @doc """
  Insert N facts in one backend transaction when supported.

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
    scoped =
      Enum.any?(items, fn item ->
        Map.get(item, :source_id) not in [nil, ""] or Map.get(item, :actor_id) not in [nil, ""]
      end)

    normalized =
      Enum.map(items, fn item ->
        %{
          content: Map.fetch!(item, :content),
          entity_ids: Map.get(item, :entity_ids, []),
          fact_type: Map.get(item, :fact_type, ""),
          tags: Map.get(item, :tags, []),
          source: Map.get(item, :source, ""),
          source_id: maybe_string(Map.get(item, :source_id, "")),
          actor_id: maybe_string(Map.get(item, :actor_id, "")),
          confidence: :erlang.float(Map.get(item, :confidence, 0.0))
        }
      end)

    if scoped do
      Native.fact_add_many_scoped(handle, normalized)
    else
      Native.fact_add_many(handle, Enum.map(normalized, &Map.drop(&1, [:source_id, :actor_id])))
    end
  end

  def fact_get(handle, fact_id, opts \\ []) do
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.fact_get(handle, fact_id)
      else
        Native.fact_get_scoped(handle, fact_id, to_string(source_id))
      end

    Jason.decode!(result)
  end

  def pinned_facts(handle, opts \\ []) do
    limit = Keyword.get(opts, :limit, 10)
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.pinned_facts(handle, limit)
      else
        Native.pinned_facts_scoped(handle, limit, to_string(source_id))
      end

    Jason.decode!(result)
  end

  def fact_list(handle, opts \\ []) do
    entity = Keyword.get(opts, :entity, "")
    type = Keyword.get(opts, :fact_type, "")
    limit = Keyword.get(opts, :limit, 0)
    current_only = Keyword.get(opts, :current_only, false)
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.fact_list(handle, entity, type, limit, current_only)
      else
        Native.fact_list_scoped(
          handle,
          entity,
          type,
          limit,
          current_only,
          to_string(source_id)
        )
      end

    Jason.decode!(result)
  end

  def fact_delete(handle, fact_id), do: Native.fact_delete(handle, fact_id)

  def fact_supersede(handle, old_id, new_id),
    do: Native.fact_supersede(handle, old_id, new_id)

  # === Relations ===========================================================

  def relation_add(handle, source, target, rel_type, opts \\ []) do
    source_id = Keyword.get(opts, :source_id, "")

    if source_id in ["", nil] do
      Native.relation_add(handle, source, target, rel_type)
    else
      Native.relation_add_scoped(handle, source, target, rel_type, to_string(source_id))
    end
  end

  def relation_remove(handle, source, target, rel_type),
    do: Native.relation_remove(handle, source, target, rel_type)

  def relations_get(handle, entity, opts \\ []) do
    direction = Keyword.get(opts, :direction, "both")
    source_id = Keyword.get(opts, :source_id, "")

    result =
      if source_id in ["", nil] do
        Native.relations_get(handle, entity, direction)
      else
        Native.relations_get_scoped(handle, entity, direction, to_string(source_id))
      end

    Jason.decode!(result)
  end

  # === Ingest / Lint / Stats ==============================================

  def ingest(handle, wiki_root, opts \\ []) do
    incremental = Keyword.get(opts, :incremental, false)
    handle |> Native.ingest(wiki_root, incremental) |> Jason.decode!()
  end

  def lint(handle), do: handle |> Native.lint() |> Jason.decode!()
  def stats(handle), do: handle |> Native.stats() |> Jason.decode!()

  # === Branch logs =========================================================

  @doc """
  Export this open store's append-only branch segment to a local directory.

  Use `base: "/path/to/backup-id"` to export only records after that backup's
  sync cursor. S3 branch URIs are intentionally handled by the CLI build that
  enables the optional `s3` Cargo feature.
  """
  def branch_push(handle, branch, destination, opts \\ []) do
    base = Keyword.get(opts, :base, "")

    if s3_uri?(destination) or s3_uri?(base) do
      raise ArgumentError, "S3 branch logs must use the aidememo CLI built with the s3 feature"
    end

    handle
    |> Native.branch_push(to_string(branch), to_string(destination), maybe_string(base))
    |> Jason.decode!()
  end

  @doc """
  Merge local branch segments into this open store.

  Pass `branch: "candidate-b"` to import only one branch. Omitting `branch`
  imports every local branch under the source directory.
  """
  def branch_merge(handle, source, opts \\ []) do
    branch = Keyword.get(opts, :branch, "")

    if s3_uri?(source) do
      raise ArgumentError, "S3 branch logs must use the aidememo CLI built with the s3 feature"
    end

    handle
    |> Native.branch_merge(to_string(source), maybe_string(branch))
    |> Jason.decode!()
  end

  defp maybe_string(nil), do: ""
  defp maybe_string(value), do: to_string(value)

  defp s3_uri?(nil), do: false
  defp s3_uri?(value), do: String.starts_with?(to_string(value), "s3://")
end
