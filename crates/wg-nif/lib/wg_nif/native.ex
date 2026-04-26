defmodule WgNif.Native do
  @moduledoc false
  # Low-level NIF stubs. Loaded from priv/libwg_nif.{so,dylib}.
  # Replaced at runtime by :erlang.load_nif/2.

  @on_load :load_nif

  @doc false
  def load_nif do
    priv =
      case :code.priv_dir(:wg_nif) do
        {:error, :bad_name} ->
          Path.join(__DIR__, "../../priv") |> Path.expand()

        path ->
          List.to_string(path)
      end

    path = Path.join(priv, "libwg_nif") |> String.to_charlist()
    :erlang.load_nif(path, 0)
  end

  def open(_path), do: :erlang.nif_error(:nif_not_loaded)
  def search(_h, _query, _limit, _current_only), do: :erlang.nif_error(:nif_not_loaded)

  def query(_h, _topic, _limit, _depth, _recent_limit, _current_only, _mode),
    do: :erlang.nif_error(:nif_not_loaded)

  def traverse(_h, _entity, _depth, _direction), do: :erlang.nif_error(:nif_not_loaded)
  def path_find(_h, _from, _to), do: :erlang.nif_error(:nif_not_loaded)

  def entity_add(_h, _name, _type, _tags, _aliases, _source_page),
    do: :erlang.nif_error(:nif_not_loaded)

  def entity_get(_h, _name), do: :erlang.nif_error(:nif_not_loaded)
  def entity_list(_h, _limit, _type), do: :erlang.nif_error(:nif_not_loaded)
  def entity_delete(_h, _name), do: :erlang.nif_error(:nif_not_loaded)
  def entity_describe(_h, _name, _summary), do: :erlang.nif_error(:nif_not_loaded)
  def resolve_entity(_h, _name), do: :erlang.nif_error(:nif_not_loaded)

  def fact_add(_h, _content, _ids, _type, _tags, _source, _confidence),
    do: :erlang.nif_error(:nif_not_loaded)

  def fact_get(_h, _id), do: :erlang.nif_error(:nif_not_loaded)

  def fact_list(_h, _entity, _type, _limit, _current_only),
    do: :erlang.nif_error(:nif_not_loaded)

  def fact_delete(_h, _id), do: :erlang.nif_error(:nif_not_loaded)
  def fact_supersede(_h, _old, _new), do: :erlang.nif_error(:nif_not_loaded)
  def relation_add(_h, _src, _tgt, _type), do: :erlang.nif_error(:nif_not_loaded)
  def relation_remove(_h, _src, _tgt, _type), do: :erlang.nif_error(:nif_not_loaded)
  def relations_get(_h, _entity, _direction), do: :erlang.nif_error(:nif_not_loaded)
  def ingest(_h, _root, _incremental), do: :erlang.nif_error(:nif_not_loaded)
  def lint(_h), do: :erlang.nif_error(:nif_not_loaded)
  def stats(_h), do: :erlang.nif_error(:nif_not_loaded)
  def version, do: :erlang.nif_error(:nif_not_loaded)
end
