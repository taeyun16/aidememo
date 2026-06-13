//! Compare the current redb-backed AideMemo store with a SQLite prototype schema.
//!
//! Scope:
//! - redb rows use the real `AideMemo` public API.
//! - SQLite rows use a minimal candidate schema with normalized entities,
//!   fact/entity join rows, and FTS5 for content search.
//! - This is a storage-shape probe, not a complete backend implementation.
//!
//! Run:
//!   cargo run --release -p aidememo-benchmarks --bin storage_backend_probe
//!   AIDEMEMO_STORAGE_BENCH_LARGE=1 cargo run --release -p aidememo-benchmarks --bin storage_backend_probe

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use aidememo_core::{
    AideMemo, Config, EntityInput, EntityType, FactInput, FactListOpts, FactType, SearchOpts,
};
use rusqlite::{Connection, params};
use serde::Serialize;
use tempfile::TempDir;

const BATCH: usize = 100;
const SINGLE_WRITES: usize = 80;

#[derive(Debug, Clone, Copy)]
enum Durability {
    Strict,
    Relaxed,
}

impl Durability {
    fn redb_label(self) -> &'static str {
        match self {
            Self::Strict => "redb_immediate",
            Self::Relaxed => "redb_eventual",
        }
    }

    fn sqlite_label(self) -> &'static str {
        match self {
            Self::Strict => "sqlite_wal_full",
            Self::Relaxed => "sqlite_wal_normal",
        }
    }

    fn redb_config_value(self) -> &'static str {
        match self {
            Self::Strict => "immediate",
            Self::Relaxed => "eventual",
        }
    }

    fn sqlite_synchronous(self) -> &'static str {
        match self {
            Self::Strict => "FULL",
            Self::Relaxed => "NORMAL",
        }
    }
}

#[derive(Debug, Serialize)]
struct Stats {
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    mean_ms: f64,
    min_ms: f64,
    max_ms: f64,
    n: usize,
}

#[derive(Debug, Serialize)]
struct Row {
    backend: String,
    scale: usize,
    op: &'static str,
    stats: Stats,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn stats(times_ms: &[f64]) -> Stats {
    let mut sorted = times_ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = times_ms.iter().sum::<f64>() / times_ms.len() as f64;
    Stats {
        p50_ms: percentile(&sorted, 0.50),
        p95_ms: percentile(&sorted, 0.95),
        p99_ms: percentile(&sorted, 0.99),
        mean_ms: mean,
        min_ms: *sorted.first().unwrap_or(&f64::NAN),
        max_ms: *sorted.last().unwrap_or(&f64::NAN),
        n: times_ms.len(),
    }
}

fn time_n<F>(n: usize, mut f: F) -> Vec<f64>
where
    F: FnMut(),
{
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let started = Instant::now();
        f();
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    times
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn entity_count(scale: usize) -> usize {
    (scale / 20).max(10)
}

fn topic(i: usize) -> &'static str {
    const TOPICS: [&str; 10] = [
        "cache",
        "persistence",
        "replication",
        "latency",
        "throughput",
        "failover",
        "consistency",
        "sharding",
        "indexing",
        "compression",
    ];
    TOPICS[i % TOPICS.len()]
}

fn fact_content(i: usize, entities: usize) -> String {
    format!(
        "Fact {i} on Entity_{} discusses {} with notes on tuning, timeouts, and failure modes.",
        i % entities,
        topic(i)
    )
}

fn redb_config(path: &Path, durability: Durability) -> Config {
    let mut config = Config::default();
    config.store.path = path.to_string_lossy().into_owned();
    config.store.durability = durability.redb_config_value().to_string();
    config.search.semantic_index = "naive".to_string();
    config
}

fn build_redb(path: &Path, scale: usize, durability: Durability) -> AideMemo {
    let config = redb_config(path, durability);
    let wiki = AideMemo::open(path, config).expect("open redb store");
    let entities = entity_count(scale);
    let mut entity_ids = Vec::with_capacity(entities);

    for i in 0..entities {
        let id = wiki
            .entity_add(EntityInput {
                name: format!("Entity_{i}"),
                entity_type: Some(EntityType::Technology),
                ..Default::default()
            })
            .expect("redb entity_add");
        entity_ids.push(id);
    }

    for chunk_start in (0..scale).step_by(1_000) {
        let chunk_end = (chunk_start + 1_000).min(scale);
        let inputs: Vec<FactInput> = (chunk_start..chunk_end)
            .map(|i| FactInput {
                content: fact_content(i, entities),
                fact_type: Some(if i % 4 == 0 {
                    FactType::Decision
                } else {
                    FactType::Note
                }),
                entity_ids: Some(vec![entity_ids[i % entities]]),
                source_confidence: Some(0.8),
                ..Default::default()
            })
            .collect();
        wiki.fact_add_many(inputs).expect("redb fact_add_many seed");
    }

    wiki
}

struct SqliteStore {
    conn: Connection,
    path: PathBuf,
}

impl SqliteStore {
    fn open(path: &Path, durability: Durability) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", durability.sqlite_synchronous())?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                name_lower TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS facts (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                fact_type TEXT NOT NULL,
                source_confidence REAL NOT NULL,
                relevance_score REAL NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                observed_at INTEGER,
                superseded_at INTEGER,
                superseded_by TEXT,
                source_id TEXT,
                pinned INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS fact_entities (
                entity_id TEXT NOT NULL,
                fact_id TEXT NOT NULL,
                PRIMARY KEY (entity_id, fact_id),
                FOREIGN KEY(entity_id) REFERENCES entities(id),
                FOREIGN KEY(fact_id) REFERENCES facts(id)
            );

            CREATE INDEX IF NOT EXISTS idx_fact_entities_fact
                ON fact_entities(fact_id);
            CREATE INDEX IF NOT EXISTS idx_facts_type
                ON facts(fact_type);
            CREATE INDEX IF NOT EXISTS idx_facts_source_id
                ON facts(source_id);
            CREATE INDEX IF NOT EXISTS idx_facts_created_at
                ON facts(created_at);

            CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts
                USING fts5(content, content='facts', content_rowid='rowid');
            "#,
        )?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    fn insert_entities(&mut self, count: usize) -> rusqlite::Result<Vec<String>> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO entities (id, name, name_lower, entity_type, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'technology', ?4, ?4)",
            )?;
            let now = now_ms() as i64;
            for i in 0..count {
                let name = format!("Entity_{i}");
                stmt.execute(params![
                    sqlite_entity_id(i),
                    name,
                    format!("entity_{i}"),
                    now
                ])?;
            }
        }
        tx.commit()?;
        Ok((0..count).map(sqlite_entity_id).collect())
    }

    fn insert_facts(
        &mut self,
        start: usize,
        count: usize,
        entities: usize,
    ) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut fact_stmt = tx.prepare(
                "INSERT INTO facts (
                    id, content, fact_type, source_confidence, relevance_score,
                    created_at, updated_at, pinned
                 )
                 VALUES (?1, ?2, ?3, 0.8, 0.5, ?4, ?4, 0)",
            )?;
            let mut link_stmt =
                tx.prepare("INSERT INTO fact_entities (entity_id, fact_id) VALUES (?1, ?2)")?;
            let mut fts_stmt = tx.prepare(
                "INSERT INTO facts_fts(rowid, content) VALUES (last_insert_rowid(), ?1)",
            )?;
            let now = now_ms() as i64;
            for i in start..(start + count) {
                let fact_id = sqlite_fact_id(i);
                let content = fact_content(i, entities);
                let fact_type = if i % 4 == 0 { "decision" } else { "note" };
                fact_stmt.execute(params![fact_id, content, fact_type, now])?;
                link_stmt.execute(params![sqlite_entity_id(i % entities), fact_id])?;
                fts_stmt.execute(params![content])?;
            }
        }
        tx.commit()
    }

    fn count_all_facts(&self) -> rusqlite::Result<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, content, fact_type FROM facts")?;
        let mut rows = stmt.query([])?;
        let mut count = 0usize;
        while rows.next()?.is_some() {
            count += 1;
        }
        Ok(count)
    }

    fn count_entity_facts(&self, entity_id: &str) -> rusqlite::Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.content
             FROM fact_entities fe
             JOIN facts f ON f.id = fe.fact_id
             WHERE fe.entity_id = ?1",
        )?;
        let mut rows = stmt.query(params![entity_id])?;
        let mut count = 0usize;
        while rows.next()?.is_some() {
            count += 1;
        }
        Ok(count)
    }

    fn search_fts(&self, query: &str) -> rusqlite::Result<usize> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.content
             FROM facts_fts
             JOIN facts f ON f.rowid = facts_fts.rowid
             WHERE facts_fts MATCH ?1
             ORDER BY bm25(facts_fts)
             LIMIT 10",
        )?;
        let mut rows = stmt.query(params![query])?;
        let mut count = 0usize;
        while rows.next()?.is_some() {
            count += 1;
        }
        Ok(count)
    }
}

fn sqlite_entity_id(i: usize) -> String {
    format!("entity-{i:08}")
}

fn sqlite_fact_id(i: usize) -> String {
    format!("fact-{i:08}")
}

fn build_sqlite(path: &Path, scale: usize, durability: Durability) -> SqliteStore {
    let mut store = SqliteStore::open(path, durability).expect("open sqlite store");
    let entities = entity_count(scale);
    store
        .insert_entities(entities)
        .expect("sqlite seed entities");
    for chunk_start in (0..scale).step_by(1_000) {
        let chunk_end = (chunk_start + 1_000).min(scale);
        store
            .insert_facts(chunk_start, chunk_end - chunk_start, entities)
            .expect("sqlite seed facts");
    }
    store
}

fn push_row(rows: &mut Vec<Row>, backend: &str, scale: usize, op: &'static str, times: Vec<f64>) {
    rows.push(Row {
        backend: backend.to_string(),
        scale,
        op,
        stats: stats(&times),
    });
}

fn run_redb(scale: usize, durability: Durability, rows: &mut Vec<Row>) {
    let backend = durability.redb_label();
    let dir = TempDir::new().expect("tempdir");
    let store_path = dir.path().join("wiki.redb");

    eprint!("  {backend}: build... ");
    let started = Instant::now();
    let wiki = build_redb(&store_path, scale, durability);
    push_row(
        rows,
        backend,
        scale,
        "seed_build",
        vec![started.elapsed().as_secs_f64() * 1000.0],
    );
    eprintln!("{:.2}s", started.elapsed().as_secs_f64());

    let entities = entity_count(scale);
    let owner = wiki
        .resolve_entity(&format!("Entity_{}", entities / 2))
        .expect("resolve owner");

    let _ = wiki
        .search("cache replication", SearchOpts::default())
        .expect("redb search warmup");

    eprint!("  {backend}: writes... ");
    let single_base = scale + 10_000;
    let mut single_idx = 0usize;
    push_row(
        rows,
        backend,
        scale,
        "fact_add",
        time_n(SINGLE_WRITES, || {
            let i = single_base + single_idx;
            single_idx = single_idx.wrapping_add(1);
            let _ = wiki
                .add_fact(FactInput {
                    content: fact_content(i, entities),
                    fact_type: Some(FactType::Note),
                    entity_ids: Some(vec![owner]),
                    source_confidence: Some(0.5),
                    ..Default::default()
                })
                .expect("redb fact_add");
        }),
    );

    let mut batch_idx = scale + 20_000;
    let batch_times: Vec<f64> = time_n(20, || {
        let inputs: Vec<FactInput> = (0..BATCH)
            .map(|offset| {
                let i = batch_idx + offset;
                FactInput {
                    content: fact_content(i, entities),
                    fact_type: Some(FactType::Note),
                    entity_ids: Some(vec![owner]),
                    source_confidence: Some(0.5),
                    ..Default::default()
                }
            })
            .collect();
        batch_idx += BATCH;
        let _ = wiki.fact_add_many(inputs).expect("redb fact_add_many");
    })
    .into_iter()
    .map(|ms| ms / BATCH as f64)
    .collect();
    push_row(rows, backend, scale, "fact_add_many_per_fact", batch_times);
    eprintln!("done");

    eprint!("  {backend}: reads/search... ");
    push_row(
        rows,
        backend,
        scale,
        "fact_list_all",
        time_n(30, || {
            let _ = wiki
                .fact_list(FactListOpts::default())
                .expect("redb fact_list_all");
        }),
    );
    push_row(
        rows,
        backend,
        scale,
        "fact_list_entity",
        time_n(80, || {
            let _ = wiki
                .fact_list(FactListOpts {
                    entity_id: Some(owner),
                    ..Default::default()
                })
                .expect("redb fact_list_entity");
        }),
    );
    push_row(
        rows,
        backend,
        scale,
        "search_bm25",
        time_n(100, || {
            let _ = wiki
                .search(
                    "cache replication",
                    SearchOpts {
                        limit: Some(10),
                        ..Default::default()
                    },
                )
                .expect("redb search");
        }),
    );
    eprintln!("done");

    eprint!("  {backend}: reopen... ");
    drop(wiki);
    push_row(
        rows,
        backend,
        scale,
        "open_existing",
        time_n(20, || {
            let wiki =
                AideMemo::open(&store_path, redb_config(&store_path, durability)).expect("reopen");
            drop(wiki);
        }),
    );
    eprintln!("done");
}

fn run_sqlite(scale: usize, durability: Durability, rows: &mut Vec<Row>) {
    let backend = durability.sqlite_label();
    let dir = TempDir::new().expect("tempdir");
    let store_path = dir.path().join("wiki.sqlite");

    eprint!("  {backend}: build... ");
    let started = Instant::now();
    let mut store = build_sqlite(&store_path, scale, durability);
    push_row(
        rows,
        backend,
        scale,
        "seed_build",
        vec![started.elapsed().as_secs_f64() * 1000.0],
    );
    eprintln!("{:.2}s", started.elapsed().as_secs_f64());

    let entities = entity_count(scale);
    let owner = sqlite_entity_id(entities / 2);
    let _ = store.search_fts("cache replication").expect("fts warmup");

    eprint!("  {backend}: writes... ");
    let mut single_idx = scale + 10_000;
    push_row(
        rows,
        backend,
        scale,
        "fact_add",
        time_n(SINGLE_WRITES, || {
            store
                .insert_facts(single_idx, 1, entities)
                .expect("sqlite fact_add");
            single_idx += 1;
        }),
    );
    let mut batch_idx = scale + 20_000;
    let batch_times: Vec<f64> = time_n(20, || {
        store
            .insert_facts(batch_idx, BATCH, entities)
            .expect("sqlite fact_add_many");
        batch_idx += BATCH;
    })
    .into_iter()
    .map(|ms| ms / BATCH as f64)
    .collect();
    push_row(rows, backend, scale, "fact_add_many_per_fact", batch_times);
    eprintln!("done");

    eprint!("  {backend}: reads/search... ");
    push_row(
        rows,
        backend,
        scale,
        "fact_list_all",
        time_n(30, || {
            let _ = store.count_all_facts().expect("sqlite fact_list_all");
        }),
    );
    push_row(
        rows,
        backend,
        scale,
        "fact_list_entity",
        time_n(80, || {
            let _ = store
                .count_entity_facts(&owner)
                .expect("sqlite fact_list_entity");
        }),
    );
    push_row(
        rows,
        backend,
        scale,
        "search_fts",
        time_n(100, || {
            let _ = store.search_fts("cache replication").expect("sqlite fts");
        }),
    );
    eprintln!("done");

    eprint!("  {backend}: reopen... ");
    drop(store);
    push_row(
        rows,
        backend,
        scale,
        "open_existing",
        time_n(20, || {
            let store = SqliteStore::open(&store_path, durability).expect("sqlite reopen");
            let _ = store.path.as_path();
            drop(store);
        }),
    );
    eprintln!("done");
}

fn print_table(rows: &[Row]) {
    println!();
    println!(
        "{:<18} {:>8}  {:<24} {:>10} {:>10} {:>10} {:>10} {:>5}",
        "backend", "scale", "op", "p50_ms", "p95_ms", "p99_ms", "mean_ms", "n"
    );
    println!("{}", "-".repeat(104));
    for row in rows {
        println!(
            "{:<18} {:>8}  {:<24} {:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>5}",
            row.backend,
            row.scale,
            row.op,
            row.stats.p50_ms,
            row.stats.p95_ms,
            row.stats.p99_ms,
            row.stats.mean_ms,
            row.stats.n
        );
    }
}

fn main() {
    let mut scales = vec![1_000, 10_000];
    if std::env::var("AIDEMEMO_STORAGE_BENCH_LARGE").is_ok() {
        scales.push(50_000);
    }

    let mut rows = Vec::new();
    for scale in scales {
        eprintln!("=== scale = {scale} ===");
        run_redb(scale, Durability::Strict, &mut rows);
        run_redb(scale, Durability::Relaxed, &mut rows);
        run_sqlite(scale, Durability::Strict, &mut rows);
        run_sqlite(scale, Durability::Relaxed, &mut rows);
    }

    print_table(&rows);

    let out_dir = Path::new("benchmarks/results");
    std::fs::create_dir_all(out_dir).expect("create results dir");
    let out_path = out_dir.join("storage_backend_probe.json");
    let json = serde_json::to_string_pretty(&rows).expect("serialize rows");
    let mut file = std::fs::File::create(&out_path).expect("create result json");
    file.write_all(json.as_bytes()).expect("write result json");
    eprintln!("\nWrote {}", out_path.display());
}
