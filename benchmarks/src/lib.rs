use criterion::{BatchSize, Criterion};
use std::fs;
use std::hint::black_box;
use std::path::Path;
use tempfile::TempDir;
use wg_core::search::SearchEngine;
use wg_core::{
    Config, EntityInput, EntityType, FactInput, FactType, SearchOpts, WikiGraph,
};
use wg_core::store::Store;

fn seed_retrieval_store(store: &mut Store) {
    let redis_id = store
        .entity_add(EntityInput {
            name: "Redis".to_string(),
            entity_type: Some(EntityType::Technology),
            aliases: Some(vec!["Redis DB".to_string()]),
            tags: Some(vec!["cache".to_string(), "infra".to_string()]),
            source_page: Some("entities/redis.md".to_string()),
        })
        .expect("seed Redis entity");

    let postgres_id = store
        .entity_add(EntityInput {
            name: "Postgres".to_string(),
            entity_type: Some(EntityType::Technology),
            aliases: Some(vec!["PostgreSQL".to_string()]),
            tags: Some(vec!["database".to_string()]),
            source_page: Some("entities/postgres.md".to_string()),
        })
        .expect("seed Postgres entity");

    let cache_id = store
        .entity_add(EntityInput {
            name: "Cache".to_string(),
            entity_type: Some(EntityType::Concept),
            tags: Some(vec!["performance".to_string()]),
            source_page: Some("concepts/cache.md".to_string()),
            ..Default::default()
        })
        .expect("seed Cache entity");

    store
        .fact_add(FactInput {
            content: "Redis Sentinel provides high availability for in-memory data stores."
                .to_string(),
            fact_type: Some(FactType::Decision),
            entity_ids: Some(vec![redis_id]),
            tags: Some(vec!["ha".to_string()]),
            source: Some("entities/redis.md#high-availability".to_string()),
            source_confidence: Some(0.9),
        })
        .expect("seed fact");

    store
        .fact_add(FactInput {
            content: "Redis Cluster provides horizontal scaling and sharding.".to_string(),
            fact_type: Some(FactType::Pattern),
            entity_ids: Some(vec![redis_id]),
            tags: Some(vec!["scaling".to_string()]),
            source: Some("entities/redis.md#scaling".to_string()),
            source_confidence: Some(0.9),
        })
        .expect("seed fact");

    store
        .fact_add(FactInput {
            content: "Postgres is used as the system of record for durable data.".to_string(),
            fact_type: Some(FactType::Claim),
            entity_ids: Some(vec![postgres_id]),
            tags: Some(vec!["storage".to_string()]),
            source: Some("entities/postgres.md#durability".to_string()),
            source_confidence: Some(0.9),
        })
        .expect("seed fact");

    store
        .fact_add(FactInput {
            content: "Caches reduce latency for repeated reads.".to_string(),
            fact_type: Some(FactType::Note),
            entity_ids: Some(vec![cache_id]),
            tags: Some(vec!["performance".to_string()]),
            source: Some("concepts/cache.md#latency".to_string()),
            source_confidence: Some(0.9),
        })
        .expect("seed fact");
}

fn create_seeded_store() -> (TempDir, Store, Config) {
    let dir = TempDir::new().expect("create temp dir");
    let path = dir.path().join("wg.redb");
    let config = Config::default();
    let mut store = Store::open(&path, config.clone()).expect("open store");
    seed_retrieval_store(&mut store);
    (dir, store, config)
}

fn write_small_wiki(root: &Path) {
    fs::create_dir_all(root.join("entities")).expect("create wiki dir");

    fs::write(
        root.join("entities/redis.md"),
        r#"---
type: technology
tags:
  - cache
  - infra
aliases:
  - Redis DB
---
# Redis

[[Cache]] powers fast lookups.

## Decision: High availability
Redis Sentinel provides high availability.

## Pattern: Scaling
Redis Cluster provides horizontal scaling.
"#,
    )
    .expect("write redis page");

    fs::write(
        root.join("entities/cache.md"),
        r#"---
type: concept
---
# Cache

## Note: Latency
Caching reduces latency for repeated reads.
"#,
    )
    .expect("write cache page");
}

pub fn bm25_search(c: &mut Criterion) {
    let (_dir, store, config) = create_seeded_store();
    let engine = SearchEngine::new(&store, &config);
    let query = black_box("high availability");

    c.bench_function("bm25_search", |b| {
        b.iter(|| {
            black_box(
                engine
                    .search(query, SearchOpts::default())
                    .expect("bm25 search"),
            )
        });
    });
}

pub fn store_open(c: &mut Criterion) {
    c.bench_function("store_open", |b| {
        b.iter_batched(
            || TempDir::new().expect("tempdir"),
            |dir| {
                let path = dir.path().join("wg.redb");
                let store = Store::open(&path, Config::default()).expect("open store");
                black_box(store);
            },
            BatchSize::SmallInput,
        );
    });
}

pub fn ingest_small(c: &mut Criterion) {
    c.bench_function("ingest_small", |b| {
        b.iter_batched(
            || {
                let wiki_dir = TempDir::new().expect("wiki tempdir");
                write_small_wiki(wiki_dir.path());
                let store_dir = TempDir::new().expect("store tempdir");
                (wiki_dir, store_dir)
            },
            |(wiki_dir, store_dir)| {
                let store_path = store_dir.path().join("wg.redb");
                let mut graph = WikiGraph::open(&store_path, Config::default()).expect("open graph");
                let stats = graph
                    .ingest(wiki_dir.path(), false)
                    .expect("ingest small wiki");
                black_box(stats);
            },
            BatchSize::SmallInput,
        );
    });
}
