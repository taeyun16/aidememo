defmodule Mix.Tasks.Compile.Cargo do
  @moduledoc false
  use Mix.Task.Compiler

  @impl true
  def run(_args) do
    crate_dir = Path.expand(__DIR__)
    workspace = Path.expand("../..", __DIR__)
    profile_args = if Mix.env() == :prod, do: ["--release"], else: []
    target_dir = if Mix.env() == :prod, do: "release", else: "debug"

    {output, status} =
      System.cmd("cargo", ["build", "-p", "wg-nif"] ++ profile_args,
        cd: workspace,
        stderr_to_stdout: true
      )

    if status != 0 do
      Mix.raise("cargo build -p wg-nif failed:\n#{output}")
    end

    priv = Path.join(crate_dir, "priv")
    File.mkdir_p!(priv)

    src_ext = if :os.type() == {:unix, :darwin}, do: "dylib", else: "so"
    src = Path.join([workspace, "target", target_dir, "libwg_nif.#{src_ext}"])
    # Erlang's :erlang.load_nif expects .so on every Unix (incl. macOS).
    dst = Path.join(priv, "libwg_nif.so")

    case File.cp(src, dst) do
      :ok ->
        {:ok, []}

      {:error, reason} ->
        Mix.raise("failed to copy #{src} → #{dst}: #{inspect(reason)}")
    end
  end
end

defmodule WgNif.MixProject do
  use Mix.Project

  def project do
    [
      app: :wg_nif,
      version: "0.1.0",
      elixir: "~> 1.15",
      description: "Elixir bindings for Wiki-Graph (wg) — local knowledge graph for LLM agents",
      package: package(),
      deps: deps(),
      compilers: [:cargo | Mix.compilers()]
    ]
  end

  def application, do: [extra_applications: [:logger]]

  defp deps, do: [{:jason, "~> 1.4"}]

  defp package do
    [
      licenses: ["MIT", "Apache-2.0"],
      links: %{"GitHub" => "https://github.com/taeyun16/wg"},
      files: ~w(lib mix.exs priv README* LICENSE*)
    ]
  end
end
