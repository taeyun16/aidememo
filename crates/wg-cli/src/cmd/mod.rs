//! CLI command definitions using bpaf.

use bpaf::*;
use std::path::PathBuf;

pub mod adapt;
pub mod bench;
pub mod completions;
pub mod daemon;
pub mod doctor;
pub mod edit;
pub mod feedback;
pub mod graph;
pub mod init;
pub mod mcp_install;
pub mod mcp_serve;
pub mod mcp_stdio;
pub mod mcp_tools;
pub mod memprobe;
pub mod model;
pub mod pending;
pub mod project;
pub mod recent;
pub mod skill;
pub mod watch;

pub use adapt::AdaptSub;
pub use bench::BenchSub;
pub use completions::CompletionsSub;
pub use doctor::DoctorSub;
pub use edit::EditSub;
pub use feedback::FeedbackSub;
pub use graph::GraphSub;
pub use init::InitSub;
pub use mcp_install::McpInstallSub;
pub use mcp_serve::McpSub;
pub use mcp_stdio::McpStdioSub;
pub use model::ModelSub;
pub use pending::PendingSub;
pub use project::ProjectSub;
pub use recent::RecentSub;
pub use skill::SkillSub;
pub use watch::WatchSub;

#[derive(Debug, Clone)]
pub struct Args {
    pub store_path: Option<PathBuf>,
    pub project: Option<String>,
    pub json: bool,
    pub command: Command,
}

#[derive(Debug, Clone)]
pub enum Command {
    Entity(EntitySub),
    Fact(FactSub),
    Traverse(TraverseSub),
    Path(PathSub),
    Search(SearchSub),
    Query(QuerySub),
    Lint(LintSub),
    Doctor(DoctorSub),
    Recent(RecentSub),
    Edit(EditSub),
    Graph(GraphSub),
    Project(ProjectSub),
    Bench(BenchSub),
    Skill(SkillSub),
    Export(ExportSub),
    Import(ImportSub),
    Stats(StatsSub),
    Ingest(IngestSub),
    Sync(SyncSub),
    Config(ConfigSub),
    Model(ModelSub),
    Feedback(FeedbackSub),
    Adapt(AdaptSub),
    Init(InitSub),
    Watch(WatchSub),
    McpServe(McpSub),
    Mcp(McpStdioSub),
    McpInstall(McpInstallSub),
    Completions(CompletionsSub),
    Pending(PendingSub),
    VectorRebuild(VectorRebuildSub),
    Daemon(daemon::DaemonSub),
    Extract(ExtractSub),
    Session(SessionSub),
    AutoRelate(AutoRelateSub),
    Overview(OverviewSub),
    Consolidate(ConsolidateSub),
}

#[derive(Debug, Clone)]
pub struct ConsolidateSub {
    pub semantic_threshold: Option<f32>,
    /// Per-type TTL pairs of the form `TYPE=DAYS` (e.g. `note=30`,
    /// `question=14`). Repeatable.
    pub ttl: Vec<String>,
    pub dry_run: bool,
    pub json: bool,
    /// When set, run the GAC (Geometry-Aware Consolidation)
    /// analysis pass instead of the pairwise + TTL pass. Stage 2a
    /// is analysis-only — emits cluster / tight-vs-spread stats
    /// without mutating the store.
    pub gac: bool,
    /// θ for GAC (retrieval half-angle). Defaults to 0.85, the
    /// paper's example value.
    pub gac_theta: Option<f32>,
    /// Spread-cluster residual budget — how many cluster members
    /// to keep on top of the medoid. 0 = medoid only.
    pub gac_spread_budget: Option<usize>,
    /// Move losers to cold-tier archive instead of superseding.
    pub gac_cold_tier: bool,
}

#[derive(Debug, Clone)]
pub struct AutoRelateSub {
    pub threshold: Option<f32>,
    pub top_k: Option<usize>,
    pub dry_run: bool,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct OverviewSub {
    pub top_n: Option<usize>,
    pub recent_days: Option<u64>,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct ExtractSub {
    pub apply: bool,
    pub min_confidence: Option<f32>,
    pub max_candidates: Option<usize>,
    pub llm: bool,
    pub from_stdin: bool,
    pub text: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SessionSub {
    /// Warmup envelope: pinned + recent + top entities + open issues.
    /// Same payload as `wg_session_start` MCP tool.
    Start {
        pinned_limit: Option<usize>,
        recent_limit: Option<usize>,
        recent_days: Option<u64>,
        top_entities_limit: Option<usize>,
    },
    /// Create a new tracked session (entity of type `session`).
    /// Prints shell-evaluable `export WG_SESSION_ID=…`. While the env
    /// var is set, every `wg fact add` auto-attaches the session
    /// entity to the new fact's entity list — that's how cross-
    /// session retrieval gets a persistent thread to follow.
    New { topic: String },
    /// Show the current session entity (per WG_SESSION_ID).
    Current,
    /// List recent session entities.
    List { limit: Option<usize> },
}

#[derive(Debug, Clone)]
pub enum EntitySub {
    Add {
        entity_type: Option<String>,
        tags: Option<Vec<String>>,
        aliases: Option<Vec<String>>,
        source_page: Option<String>,
        name: String,
    },
    Get {
        name: String,
    },
    List {
        sort: Option<String>,
        entity_type: Option<String>,
        min_facts: Option<u32>,
        limit: Option<usize>,
    },
    Rename {
        old_name: String,
        new_name: String,
    },
    Alias {
        name: String,
        alias: String,
        action: AliasAction,
    },
    Delete {
        name: String,
    },
    Describe {
        from_stdin: bool,
        clear: bool,
        name: String,
        content: Option<String>,
    },
    Show {
        recent: Option<usize>,
        name: String,
    },
}

#[derive(Debug, Clone)]
pub enum AliasAction {
    Add,
}

#[derive(Debug, Clone)]
pub enum FactSub {
    Add {
        fact_type: Option<String>,
        entities: Option<Vec<String>>,
        tags: Option<Vec<String>>,
        source: Option<String>,
        confidence: Option<f32>,
        observed_at: Option<String>,
        content: String,
    },
    Get {
        id: String,
    },
    List {
        fact_type: Option<String>,
        entity: Option<String>,
        min_confidence: Option<f32>,
        since: Option<String>,
        until: Option<String>,
        last: Option<String>,
        as_of: Option<String>,
        limit: Option<usize>,
    },
    Delete {
        id: String,
    },
    Feedback {
        helpful: bool,
        id: String,
    },
    Pin {
        id: String,
    },
    Unpin {
        id: String,
    },
    Pinned {
        limit: Option<usize>,
    },
    Supersede {
        old_id: String,
        new_id: String,
    },
    Archive {
        /// Explicit fact ids to archive (comma-separated).
        ids: Option<String>,
        /// Archive all facts older than this duration (`30d`, `4w`, `1y`).
        /// Compares observed_at if present, else created_at. Mutually
        /// compatible with --type to scope by fact_type.
        older_than: Option<String>,
        /// Optional fact_type filter when --older-than is used.
        fact_type: Option<String>,
        /// Print the candidate id list but don't move anything.
        dry_run: bool,
    },
}

#[derive(Debug, Clone)]
pub struct TraverseSub {
    pub depth: Option<u32>,
    pub entity: String,
}

#[derive(Debug, Clone)]
pub struct PathSub {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone)]
pub struct SearchSub {
    pub json: bool,
    pub all_projects: bool,
    /// Opt into hybrid BM25 + semantic ranking (loads the embedding
    /// model — adds ~700-900 ms cold-start). Default is BM25-only:
    /// the CLI is the latency-sensitive path, semantic recall is for
    /// agents that explicitly need it (or via `wg config set
    /// search.semantic_weight …` for users on languages with weak
    /// BM25 tokenisation, e.g. Korean).
    pub hybrid: bool,
    /// Optional `wg mcp-serve` endpoint (e.g. `http://localhost:3000`).
    /// When set, dispatch the search via `wg_search` JSON-RPC against
    /// that daemon instead of opening the redb store in-process.
    /// Trades the ~70 ms `wg --bm25` cold-start for ~10–20 ms warm
    /// over a single HTTP round-trip — the daemon keeps the model
    /// loaded.
    pub via: Option<String>,
    pub traverse_from: Option<String>,
    pub traverse_depth: Option<u32>,
    pub min_confidence: Option<f32>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub last: Option<String>,
    pub as_of: Option<String>,
    pub limit: Option<usize>,
    /// Also search the cold-tier archive (`<store>.cold.redb`) and
    /// merge any matches in to fill out the result list. Off by
    /// default — most callers want only the live (hot) facts.
    pub include_archive: bool,
    pub query: String,
}

#[derive(Debug, Clone)]
pub struct LintSub {
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct QuerySub {
    pub limit: Option<usize>,
    pub depth: Option<u32>,
    pub recent_limit: Option<usize>,
    pub last: Option<String>,
    pub mode: Option<String>,
    pub topic: String,
}

#[derive(Debug, Clone)]
pub struct ExportSub {
    pub scope: Option<String>,
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ImportSub {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct StatsSub;

#[derive(Debug, Clone)]
pub struct VectorRebuildSub {
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct IngestSub {
    pub incremental: bool,
    pub wiki_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SyncSub {
    pub wiki_root: PathBuf,
}

#[derive(Debug, Clone)]
pub enum ConfigSub {
    List,
    Get { key: String },
    Set { key: String, value: String },
}

pub fn build_cli() -> OptionParser<Args> {
    let store_path = long("store")
        .short('s')
        .help("Path to wiki.redb store (overrides --project and config)")
        .argument::<PathBuf>("PATH")
        .optional();

    let project = long("project")
        .short('P')
        .help("Use a registered project (see `wg project list`)")
        .argument::<String>("NAME")
        .optional();

    let json = long("json")
        .short('j')
        .help("Output JSON for read commands (entity, fact, search, traverse, path, stats, lint)")
        .switch();

    let init_cmd = init::init_command();
    let watch_cmd = watch::watch_command();
    let mcp_serve_cmd = mcp_serve::mcp_serve_command();
    let mcp_cmd = mcp_stdio::mcp_command();
    let mcp_install_cmd = mcp_install::mcp_install_command();
    let completions_cmd = completions::completions_command();
    let pending_cmd = pending::pending_command();
    let model_cmd = model::model_command();
    let feedback_cmd = feedback::feedback_command();
    let adapt_cmd = adapt::adapt_command();
    let doctor_cmd = doctor::doctor_command();
    let recent_cmd = recent::recent_command();
    let edit_cmd = edit::edit_command();
    let graph_cmd = graph::graph_command();
    let project_cmd = project::project_command();
    let bench_cmd = bench::bench_command();
    let skill_cmd = skill::skill_command();
    let daemon_cmd = daemon::daemon_command();

    let command = construct!([
        entity_command(),
        fact_command(),
        traverse_command(),
        path_command(),
        search_command(),
        query_command(),
        lint_command(),
        doctor_cmd,
        recent_cmd,
        edit_cmd,
        graph_cmd,
        project_cmd,
        bench_cmd,
        skill_cmd,
        export_command(),
        import_command(),
        stats_command(),
        ingest_command(),
        sync_command(),
        config_command(),
        model_cmd,
        feedback_cmd,
        adapt_cmd,
        init_cmd,
        watch_cmd,
        mcp_serve_cmd,
        mcp_cmd,
        mcp_install_cmd,
        completions_cmd,
        pending_cmd,
        vector_rebuild_command(),
        daemon_cmd,
        extract_command(),
        session_command(),
        auto_relate_command(),
        overview_command(),
        consolidate_command(),
    ]);

    construct!(Args {
        store_path,
        project,
        json,
        command,
    })
    .to_options()
    .descr("Wiki-Graph: Structured index engine for LLM wikis")
}

fn singleton_list(parser: impl Parser<Option<String>>) -> impl Parser<Option<Vec<String>>> {
    parser.map(|value| value.map(|value| vec![value]))
}

fn extract_command() -> impl Parser<Command> {
    let apply = long("apply")
        .help("Persist every surviving candidate via fact_add (otherwise preview only)")
        .switch();
    let min_confidence = long("min-confidence")
        .help("Drop candidates below this score (0.0-1.0, default 0.5)")
        .argument::<f32>("F")
        .optional();
    let max_candidates = long("max-candidates")
        .help("Cap on candidates returned (default 20)")
        .argument::<usize>("N")
        .optional();
    let llm = long("llm")
        .help(
            "Use the configured LLM extractor (extract.provider) instead of \
             the heuristic. Falls back to heuristic with a warning if the \
             provider is unset or the call fails.",
        )
        .switch();
    let from_stdin = long("from-stdin")
        .help("Read text from stdin instead of the TEXT positional")
        .switch();
    let text = positional::<String>("TEXT").optional();

    construct!(ExtractSub {
        apply,
        min_confidence,
        max_candidates,
        llm,
        from_stdin,
        text,
    })
    .map(Command::Extract)
    .to_options()
    .command("extract")
    .help(
        "Conversation / text → candidate facts (preview by default, --apply to \
         persist). Heuristic by default; --llm dispatches to extract.provider.",
    )
}

fn session_command() -> impl Parser<Command> {
    let pinned_limit = long("pinned-limit")
        .help("Max pinned facts to surface (default 20)")
        .argument::<usize>("N")
        .optional();
    let recent_limit = long("recent-limit")
        .help("Max recent facts to surface (default 10)")
        .argument::<usize>("N")
        .optional();
    let recent_days = long("recent-days")
        .help("Lookback window for recent facts (default 7)")
        .argument::<u64>("DAYS")
        .optional();
    let top_entities_limit = long("top-entities")
        .help("Max top-entities to surface (default 10)")
        .argument::<usize>("N")
        .optional();

    let start = construct!(SessionSub::Start {
        pinned_limit,
        recent_limit,
        recent_days,
        top_entities_limit,
    })
    .to_options()
    .command("start")
    .help("Return one envelope: stats + pinned + recent + top entities + open issues");

    let topic = positional::<String>("TOPIC");
    let new_cmd = construct!(SessionSub::New { topic })
        .to_options()
        .command("new")
        .help(
            "Create a tracked session entity for TOPIC and print \
             `export WG_SESSION_ID=...`. While that env var is set, \
             every `wg fact add` auto-attaches the session entity. \
             Usage:  eval \"$(wg session new 'auth migration')\"",
        );

    let current_cmd = pure(SessionSub::Current)
        .to_options()
        .command("current")
        .help("Show the current session entity (per WG_SESSION_ID env)");

    let limit = long("limit")
        .help("Max sessions to return (default 20)")
        .argument::<usize>("N")
        .optional();
    let list_cmd = construct!(SessionSub::List { limit })
        .to_options()
        .command("list")
        .help("List recent session entities (entity_type=session)");

    construct!([start, new_cmd, current_cmd, list_cmd])
        .map(Command::Session)
        .to_options()
        .command("session")
        .help("Agent session helpers: warmup envelope + tracked sessions")
}

fn entity_command() -> impl Parser<Command> {
    let name = positional::<String>("NAME");
    let entity_type = long("type")
        .short('t')
        .help("Entity type (technology, concept, person, team)")
        .argument::<String>("TYPE")
        .optional();
    let tags = singleton_list(
        long("tags")
            .short('g')
            .help("Tags (comma-separated)")
            .argument::<String>("TAGS")
            .optional(),
    );
    let aliases = singleton_list(
        long("aliases")
            .short('a')
            .help("Aliases (comma-separated)")
            .argument::<String>("ALIASES")
            .optional(),
    );
    let source_page = long("source")
        .short('s')
        .help("Source page path")
        .argument::<String>("SOURCE")
        .optional();
    let add = construct!(EntitySub::Add {
        entity_type,
        tags,
        aliases,
        source_page,
        name,
    })
    .to_options()
    .command("add")
    .help("Add a new entity");

    let name = positional::<String>("NAME");
    let get = construct!(EntitySub::Get { name })
        .to_options()
        .command("get")
        .help("Get an entity by name");

    let sort = long("sort")
        .help("Sort by: name, fact-count, updated-at")
        .argument::<String>("SORT")
        .optional();
    let entity_type = long("type")
        .short('t')
        .help("Filter by entity type")
        .argument::<String>("TYPE")
        .optional();
    let min_facts = long("min-facts")
        .help("Minimum number of facts")
        .argument::<u32>("N")
        .optional();
    let limit = long("limit")
        .short('l')
        .help("Maximum number of results")
        .argument::<usize>("LIMIT")
        .optional();
    let list = construct!(EntitySub::List {
        sort,
        entity_type,
        min_facts,
        limit,
    })
    .to_options()
    .command("list")
    .help("List entities");

    let old_name = positional::<String>("OLD_NAME");
    let new_name = positional::<String>("NEW_NAME");
    let rename = construct!(EntitySub::Rename { old_name, new_name })
        .to_options()
        .command("rename")
        .help("Rename an entity");

    let name = positional::<String>("NAME");
    let alias = positional::<String>("ALIAS");
    let alias = construct!(name, alias)
        .map(|(name, alias)| EntitySub::Alias {
            name,
            alias,
            action: AliasAction::Add,
        })
        .to_options()
        .command("alias")
        .help("Manage entity aliases");

    let name = positional::<String>("NAME");
    let delete = construct!(EntitySub::Delete { name })
        .to_options()
        .command("delete")
        .help("Delete an entity");

    let from_stdin = long("from-stdin")
        .help("Read summary from stdin instead of CONTENT")
        .switch();
    let clear = long("clear").help("Clear the existing summary").switch();
    let name = positional::<String>("NAME");
    let content = positional::<String>("CONTENT").optional();
    let describe = construct!(EntitySub::Describe {
        from_stdin,
        clear,
        name,
        content,
    })
    .to_options()
    .command("describe")
    .help("Set the entity's compiled-truth summary (use --from-stdin or pass CONTENT)");

    let recent = long("recent")
        .short('n')
        .help("Number of recent facts to include (default 5)")
        .argument::<usize>("N")
        .optional();
    let name = positional::<String>("NAME");
    let show = construct!(EntitySub::Show { recent, name })
        .to_options()
        .command("show")
        .help("Show entity page: summary + recent facts (the compiled view)");

    construct!([add, get, list, rename, alias, delete, describe, show])
        .map(Command::Entity)
        .to_options()
        .command("entity")
        .short('e')
        .help("Entity management commands")
}

fn fact_command() -> impl Parser<Command> {
    let content = positional::<String>("CONTENT");
    let fact_type = long("type")
        .short('t')
        .help("Fact type (decision, pattern, convention, claim, note)")
        .argument::<String>("TYPE")
        .optional();
    let entities = singleton_list(
        long("entities")
            .short('e')
            .help("Entity names (comma-separated)")
            .argument::<String>("ENTITIES")
            .optional(),
    );
    let tags = singleton_list(
        long("tags")
            .short('g')
            .help("Tags (comma-separated)")
            .argument::<String>("TAGS")
            .optional(),
    );
    let source = long("source")
        .short('s')
        .help("Source page path")
        .argument::<String>("SOURCE")
        .optional();
    let confidence = long("confidence")
        .short('c')
        .help("Source confidence (0.0-1.0)")
        .argument::<f32>("CONFIDENCE")
        .optional();
    let observed_at = long("observed-at")
        .help("When the fact was actually observed/decided (YYYY-MM-DD or RFC3339)")
        .argument::<String>("DATE")
        .optional();
    let add = construct!(FactSub::Add {
        fact_type,
        entities,
        tags,
        source,
        confidence,
        observed_at,
        content,
    })
    .to_options()
    .command("add")
    .help("Add a new fact");

    let id = positional::<String>("ID");
    let get = construct!(FactSub::Get { id })
        .to_options()
        .command("get")
        .help("Get a fact by ID");

    let fact_type = long("type")
        .short('t')
        .help("Filter by fact type")
        .argument::<String>("TYPE")
        .optional();
    let entity = long("entity")
        .short('e')
        .help("Filter by entity name")
        .argument::<String>("ENTITY")
        .optional();
    let min_confidence = long("min-confidence")
        .help("Minimum source confidence")
        .argument::<f32>("CONFIDENCE")
        .optional();
    let since = long("since")
        .help("Lower-bound date (YYYY-MM-DD or RFC3339). Compares observed_at if present, else created_at.")
        .argument::<String>("DATE")
        .optional();
    let until = long("until")
        .help("Upper-bound date (YYYY-MM-DD or RFC3339)")
        .argument::<String>("DATE")
        .optional();
    let last = long("last")
        .help("Relative window from now: e.g. 30d, 12h, 4w")
        .argument::<String>("DURATION")
        .optional();
    let as_of = long("as-of")
        .help(
            "Show only facts that were *current* at this point in time \
             (YYYY-MM-DD or RFC3339). A fact qualifies if it existed by \
             then (created_at ≤ as-of) and wasn't superseded yet \
             (superseded_at > as-of or absent). Lets the caller answer \
             'what did we believe was true on that date?' without \
             walking the supersede chain.",
        )
        .argument::<String>("DATE")
        .optional();
    let limit = long("limit")
        .short('l')
        .help("Maximum number of results")
        .argument::<usize>("LIMIT")
        .optional();
    let list = construct!(FactSub::List {
        fact_type,
        entity,
        min_confidence,
        since,
        until,
        last,
        as_of,
        limit,
    })
    .to_options()
    .command("list")
    .help("List facts");

    let id = positional::<String>("ID");
    let delete = construct!(FactSub::Delete { id })
        .to_options()
        .command("delete")
        .help("Delete a fact");

    let id = positional::<String>("ID");
    let helpful = long("helpful").short('h').help("Mark as helpful").switch();
    let feedback = construct!(FactSub::Feedback { helpful, id })
        .to_options()
        .command("feedback")
        .help("Record fact feedback");

    let old_id = positional::<String>("OLD_ID");
    let new_id = positional::<String>("NEW_ID");
    let supersede = construct!(FactSub::Supersede { old_id, new_id })
        .to_options()
        .command("supersede")
        .help("Mark OLD_ID as superseded by NEW_ID (validity window)");

    let id = positional::<String>("ID");
    let pin = construct!(FactSub::Pin { id })
        .to_options()
        .command("pin")
        .help("Add a fact to the always-loaded tier (wg fact pinned, wg_pinned_context)");

    let id = positional::<String>("ID");
    let unpin = construct!(FactSub::Unpin { id })
        .to_options()
        .command("unpin")
        .help("Remove a fact from the always-loaded tier");

    let limit = long("limit")
        .short('l')
        .help("Cap on facts returned")
        .argument::<usize>("N")
        .optional();
    let pinned = construct!(FactSub::Pinned { limit })
        .to_options()
        .command("pinned")
        .help("List pinned facts (the always-loaded tier), most-recently-accessed first");

    let ids = long("ids")
        .help("Explicit fact ids (comma-separated)")
        .argument::<String>("IDS")
        .optional();
    let older_than = long("older-than")
        .help("Archive facts older than DURATION (e.g. 30d, 4w, 1y). Compares observed_at if present, else created_at.")
        .argument::<String>("DURATION")
        .optional();
    let fact_type = long("type")
        .short('t')
        .help("Optional fact_type filter when --older-than is used")
        .argument::<String>("TYPE")
        .optional();
    let dry_run = long("dry-run")
        .help("Print candidate ids without moving any fact")
        .switch();
    let archive = construct!(FactSub::Archive {
        ids,
        older_than,
        fact_type,
        dry_run,
    })
    .to_options()
    .command("archive")
    .help(
        "Move facts to the cold-tier archive (<store>.cold.redb). \
         Hot store shrinks; cold preserves FactId so wg_fact_get \
         keeps working. Use `wg search --include-archive` when you \
         want archived hits merged back into retrieval.",
    );

    construct!([
        add, get, list, delete, feedback, supersede, pin, unpin, pinned, archive
    ])
    .map(Command::Fact)
    .to_options()
    .command("fact")
    .short('f')
    .help("Fact management commands")
}

fn traverse_command() -> impl Parser<Command> {
    let entity = positional::<String>("ENTITY");
    let depth = long("depth")
        .short('d')
        .help("Maximum traversal depth")
        .argument::<u32>("DEPTH")
        .optional();

    construct!(TraverseSub { depth, entity })
        .map(Command::Traverse)
        .to_options()
        .command("traverse")
        .short('t')
        .help("Traverse the entity graph")
}

fn path_command() -> impl Parser<Command> {
    let from = positional::<String>("FROM");
    let to = positional::<String>("TO");

    construct!(PathSub { from, to })
        .map(Command::Path)
        .to_options()
        .command("path")
        .short('p')
        .help("Find a path between two entities")
}

fn search_command() -> impl Parser<Command> {
    let query = positional::<String>("QUERY");
    let traverse_from = long("traverse-from")
        .help("Start traversal from entity")
        .argument::<String>("ENTITY")
        .optional();
    let traverse_depth = long("depth")
        .short('d')
        .help("Traversal depth for search scope")
        .argument::<u32>("DEPTH")
        .optional();
    let min_confidence = long("min-confidence")
        .help("Minimum source confidence")
        .argument::<f32>("CONFIDENCE")
        .optional();
    let since = long("since")
        .help("Lower-bound date (YYYY-MM-DD or RFC3339). Compares observed_at if present, else created_at.")
        .argument::<String>("DATE")
        .optional();
    let until = long("until")
        .help("Upper-bound date (YYYY-MM-DD or RFC3339)")
        .argument::<String>("DATE")
        .optional();
    let last = long("last")
        .help("Relative window from now: e.g. 30d, 12h, 4w")
        .argument::<String>("DURATION")
        .optional();
    let as_of = long("as-of")
        .help(
            "Restrict results to facts current at this date (YYYY-MM-DD \
             or RFC3339). See `wg fact list --as-of` for details.",
        )
        .argument::<String>("DATE")
        .optional();
    let limit = long("limit")
        .short('l')
        .help("Maximum number of results")
        .argument::<usize>("LIMIT")
        .optional();
    let json = long("json").short('j').help("Output as JSON").switch();
    let all_projects = long("all-projects")
        .help("Search across every registered project (`wg project list`); merges + re-ranks")
        .switch();
    // The CLI defaults to BM25 (the latency-sensitive path). `--hybrid`
    // opts into the semantic-ranking path that loads the embedding
    // model (~700-900ms cold start on a fresh CLI; near-zero in
    // `--via` daemon mode). Agents reach hybrid by default through
    // the MCP tool surface, where the cost is amortised by the
    // long-lived `wg mcp-serve` process.
    let hybrid = long("hybrid")
        .help(
            "Hybrid BM25 + semantic search (loads the embedding model). \
             Pair with --via for warm daemon mode.",
        )
        .switch();
    let via = long("via")
        .help(
            "Dispatch via a running `wg mcp-serve` daemon at this URL \
             (e.g. http://localhost:3000). Skips the local redb open + \
             model load entirely; the daemon keeps both warm. Use when \
             multiple agents share a store.",
        )
        .argument::<String>("URL")
        .optional();

    let include_archive = long("include-archive")
        .help(
            "Also search the cold-tier archive (`<store>.cold.redb`) \
             and merge matches in to fill out the result list. Off by \
             default; cold facts only surface when this flag is set.",
        )
        .switch();

    construct!(SearchSub {
        json,
        all_projects,
        hybrid,
        via,
        traverse_from,
        traverse_depth,
        min_confidence,
        since,
        until,
        last,
        as_of,
        limit,
        include_archive,
        query,
    })
    .map(Command::Search)
    .to_options()
    .command("search")
    .short('s')
    .help("Search facts (use --all-projects for cross-project)")
}

fn lint_command() -> impl Parser<Command> {
    let json = long("json").short('j').help("Output as JSON").switch();

    construct!(LintSub { json })
        .map(Command::Lint)
        .to_options()
        .command("lint")
        .help("Check graph health")
}

fn query_command() -> impl Parser<Command> {
    let limit = long("limit")
        .short('l')
        .help("Search hits to include (default 10)")
        .argument::<usize>("LIMIT")
        .optional();
    let depth = long("depth")
        .short('d')
        .help("Traverse depth when topic is an entity (default 2)")
        .argument::<u32>("DEPTH")
        .optional();
    let recent_limit = long("recent-limit")
        .help("Recent facts to include (default 10)")
        .argument::<usize>("N")
        .optional();
    let last = long("last")
        .help("Restrict search/recent to last N: e.g. 30d, 12h, 4w")
        .argument::<String>("DURATION")
        .optional();
    let mode = long("mode")
        .short('m')
        .help("Retrieval strategy: naive | local | hybrid (default) | global")
        .argument::<String>("MODE")
        .optional();
    let topic = positional::<String>("TOPIC");

    construct!(QuerySub {
        limit,
        depth,
        recent_limit,
        last,
        mode,
        topic
    })
    .map(Command::Query)
    .to_options()
    .command("query")
    .short('q')
    .help("Unified context fetch (naive/local/hybrid/global modes)")
}

fn export_command() -> impl Parser<Command> {
    let scope = long("scope")
        .help("Export scope: all, entities, relations, facts")
        .argument::<String>("SCOPE")
        .optional();
    let output = long("output")
        .short('o')
        .help("Output file path")
        .argument::<PathBuf>("PATH")
        .optional();

    construct!(ExportSub { scope, output })
        .map(Command::Export)
        .to_options()
        .command("export")
        .help("Export data to JSONL")
}

fn import_command() -> impl Parser<Command> {
    let path = positional::<PathBuf>("PATH").optional();

    construct!(ImportSub { path })
        .map(Command::Import)
        .to_options()
        .command("import")
        .help("Import data from JSONL")
}

fn stats_command() -> impl Parser<Command> {
    pure(StatsSub)
        .map(Command::Stats)
        .to_options()
        .command("stats")
        .help("Show store statistics")
}

fn vector_rebuild_command() -> impl Parser<Command> {
    let json = long("json")
        .help("Emit JSON instead of human-readable output")
        .switch();

    construct!(VectorRebuildSub { json })
        .map(Command::VectorRebuild)
        .to_options()
        .command("vector-rebuild")
        .help(
            "Rebuild the HNSW vector index from scratch. \
             Use after switching embedding models or recovering \
             from a corrupted sidecar; otherwise the index is \
             refreshed lazily on ingest.",
        )
}

fn auto_relate_command() -> impl Parser<Command> {
    let threshold = long("threshold")
        .help(
            "Minimum hybrid-search score to count two facts as similar \
             (default 0.0 = off; top-k is the primary cutoff). RRF-fused \
             scores run ~0.01-0.04, BM25-only fallback runs 1-5.",
        )
        .argument::<f32>("FLOAT")
        .optional();
    let top_k = long("top-k")
        .help("Top-K similar facts to inspect per source fact (default 3)")
        .argument::<usize>("N")
        .optional();
    let dry_run = long("dry-run")
        .help("Evaluate pairs but don't write edges")
        .switch();
    let json = long("json")
        .help("Emit JSON stats instead of human-readable output")
        .switch();

    construct!(AutoRelateSub {
        threshold,
        top_k,
        dry_run,
        json,
    })
    .map(Command::AutoRelate)
    .to_options()
    .command("auto-relate")
    .help(
        "Discover entity-to-entity `related` edges from semantic \
         similarity between facts. Run after a big ingest; idempotent. \
         Requires the semantic feature (HNSW index).",
    )
}

fn overview_command() -> impl Parser<Command> {
    let top_n = long("top-n")
        .short('n')
        .help("Top-N entities per bucket and globally (default 10)")
        .argument::<usize>("N")
        .optional();
    let recent_days = long("recent-days")
        .help("Window in days for the recent-fact-count field (default 7)")
        .argument::<u64>("DAYS")
        .optional();
    let json = long("json")
        .help("Emit JSON instead of human-readable output")
        .switch();

    construct!(OverviewSub {
        top_n,
        recent_days,
        json,
    })
    .map(Command::Overview)
    .to_options()
    .command("overview")
    .help(
        "First-impression snapshot of the wiki: entity-type buckets \
         with top examples, fact-type distribution, top central \
         entities, recent activity, current/pinned/orphan counts. \
         Designed for agents arriving at an unfamiliar wiki — one call \
         instead of stats + entity list + fact list.",
    )
}

fn consolidate_command() -> impl Parser<Command> {
    let semantic_threshold = long("semantic-threshold")
        .help(
            "Pairwise cosine threshold above which the older fact is \
             marked superseded by the newer (default 0.85, OMEGA-style; \
             0.9+ for stricter dedup, 0.75 for aggressive merging). \
             Set 0.0 to disable.",
        )
        .argument::<f32>("FLOAT")
        .optional();
    let ttl = long("ttl")
        .help(
            "Per-type TTL in days, format TYPE=DAYS (e.g. --ttl note=30 \
             --ttl question=14). Facts of that type older than DAYS are \
             marked superseded with no replacement (expiry, not \
             dedup). Repeatable. Types not listed stay permanent.",
        )
        .argument::<String>("TYPE=DAYS")
        .many();
    let dry_run = long("dry-run")
        .help("Evaluate pairs / TTL and report stats but don't write any supersedes")
        .switch();
    let json = long("json")
        .help("Emit JSON stats instead of human-readable output")
        .switch();
    let gac = long("gac")
        .help(
            "Run GAC (Geometry-Aware Consolidation) analysis instead of \
             pairwise + TTL. Stage 2a: analysis-only — clusters, d̄, \
             tight vs spread classification. No store mutation.",
        )
        .switch();
    let gac_theta = long("gac-theta")
        .help("θ for GAC (retrieval half-angle). Defaults to 0.85.")
        .argument::<f32>("FLOAT")
        .optional();
    let gac_spread_budget = long("gac-spread-budget")
        .help(
            "How many spread-cluster members to keep on top of the medoid. \
             0 = medoid only (most aggressive). 1+ keeps the most-distant \
             members so cluster diversity isn't lost.",
        )
        .argument::<usize>("N")
        .optional();
    let gac_cold_tier = long("gac-cold-tier")
        .help(
            "Move non-representative cluster members to <store>.cold.redb \
             instead of superseding. FactId is preserved so wg_fact_get \
             still resolves. Off by default (supersede semantics).",
        )
        .switch();

    construct!(ConsolidateSub {
        semantic_threshold,
        ttl,
        dry_run,
        json,
        gac,
        gac_theta,
        gac_spread_budget,
        gac_cold_tier,
    })
    .map(Command::Consolidate)
    .to_options()
    .command("consolidate")
    .help(
        "Periodic memory-lifecycle pass: (1) semantic dedup — pairs \
         of current facts with cosine ≥ threshold collapse to the \
         newer one, older marked superseded; (2) TTL — facts of \
         per-type configured age are marked expired. Both passes are \
         idempotent. Mirrors OMEGA's compaction + typed forgetting. \
         Requires the semantic feature.",
    )
}

fn ingest_command() -> impl Parser<Command> {
    let wiki_root = positional::<PathBuf>("WIKI_ROOT");
    let incremental = long("incremental")
        .short('i')
        .help("Incremental re-ingest")
        .switch();

    construct!(IngestSub {
        incremental,
        wiki_root,
    })
    .map(Command::Ingest)
    .to_options()
    .command("ingest")
    .help("Ingest wiki files")
}

fn sync_command() -> impl Parser<Command> {
    let wiki_root = positional::<PathBuf>("WIKI_ROOT");

    construct!(SyncSub { wiki_root })
        .map(Command::Sync)
        .to_options()
        .command("sync")
        .help("Sync wiki (incremental ingest)")
}

fn config_command() -> impl Parser<Command> {
    let list = pure(ConfigSub::List)
        .to_options()
        .command("list")
        .help("List all config");

    let key = positional::<String>("KEY");
    let get = construct!(ConfigSub::Get { key })
        .to_options()
        .command("get")
        .help("Get a config value");

    let key = positional::<String>("KEY");
    let value = positional::<String>("VALUE");
    let set = construct!(ConfigSub::Set { key, value })
        .to_options()
        .command("set")
        .help("Set a config value");

    construct!([list, get, set])
        .map(Command::Config)
        .to_options()
        .command("config")
        .help("Configuration management")
}
