//! CLI command definitions using bpaf.

use bpaf::*;
use std::path::PathBuf;

pub mod init;
pub mod watch;

pub use init::InitSub;
pub use watch::WatchSub;

#[derive(Debug, Clone)]
pub struct Args {
    pub store_path: Option<PathBuf>,
    pub command: Command,
}

#[derive(Debug, Clone)]
pub enum Command {
    Entity(EntitySub),
    Fact(FactSub),
    Traverse(TraverseSub),
    Path(PathSub),
    Search(SearchSub),
    Lint(LintSub),
    Export(ExportSub),
    Import(ImportSub),
    Stats(StatsSub),
    Ingest(IngestSub),
    Sync(SyncSub),
    Config(ConfigSub),
    Model(ModelSub),
    Init(InitSub),
    Watch(WatchSub),
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
        content: String,
    },
    Get {
        id: String,
    },
    List {
        fact_type: Option<String>,
        entity: Option<String>,
        min_confidence: Option<f32>,
        limit: Option<usize>,
    },
    Delete {
        id: String,
    },
    Feedback {
        helpful: bool,
        id: String,
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
    pub traverse_from: Option<String>,
    pub traverse_depth: Option<u32>,
    pub min_confidence: Option<f32>,
    pub limit: Option<usize>,
    pub query: String,
}

#[derive(Debug, Clone)]
pub struct LintSub {
    pub json: bool,
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

#[derive(Debug, Clone)]
pub enum ModelSub {
    Status,
    Download { name: Option<String> },
    RebuildVectors,
}

pub fn build_cli() -> OptionParser<Args> {
    let store_path = long("store")
        .short('s')
        .help("Path to wiki.redb store")
        .argument::<PathBuf>("PATH")
        .optional();

    let init_cmd = init::init_command();
    let watch_cmd = watch::watch_command();

    let command = construct!([
        entity_command(),
        fact_command(),
        traverse_command(),
        path_command(),
        search_command(),
        lint_command(),
        export_command(),
        import_command(),
        stats_command(),
        ingest_command(),
        sync_command(),
        config_command(),
        model_command(),
        init_cmd,
        watch_cmd,
    ]);

    construct!(Args {
        store_path,
        command,
    })
    .to_options()
    .descr("Wiki-Graph: Structured index engine for LLM wikis")
}

fn singleton_list(parser: impl Parser<Option<String>>) -> impl Parser<Option<Vec<String>>> {
    parser.map(|value| value.map(|value| vec![value]))
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

    construct!([add, get, list, rename, alias, delete])
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
    let add = construct!(FactSub::Add {
        fact_type,
        entities,
        tags,
        source,
        confidence,
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
    let limit = long("limit")
        .short('l')
        .help("Maximum number of results")
        .argument::<usize>("LIMIT")
        .optional();
    let list = construct!(FactSub::List {
        fact_type,
        entity,
        min_confidence,
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

    construct!([add, get, list, delete, feedback])
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
    let limit = long("limit")
        .short('l')
        .help("Maximum number of results")
        .argument::<usize>("LIMIT")
        .optional();
    let json = long("json").short('j').help("Output as JSON").switch();

    construct!(SearchSub {
        json,
        traverse_from,
        traverse_depth,
        min_confidence,
        limit,
        query,
    })
    .map(Command::Search)
    .to_options()
    .command("search")
    .short('s')
    .help("Search facts")
}

fn lint_command() -> impl Parser<Command> {
    let json = long("json").short('j').help("Output as JSON").switch();

    construct!(LintSub { json })
        .map(Command::Lint)
        .to_options()
        .command("lint")
        .help("Check graph health")
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

fn model_command() -> impl Parser<Command> {
    let status = pure(ModelSub::Status)
        .to_options()
        .command("status")
        .help("Show model status");

    let name = positional::<String>("NAME").optional();
    let download = construct!(ModelSub::Download { name })
        .to_options()
        .command("download")
        .help("Download a model");

    let rebuild_vectors = pure(ModelSub::RebuildVectors)
        .to_options()
        .command("rebuild-vectors")
        .help("Rebuild semantic vectors");

    construct!([status, download, rebuild_vectors])
        .map(Command::Model)
        .to_options()
        .command("model")
        .help("Model management")
}
