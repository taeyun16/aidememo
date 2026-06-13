//! Integration test: locks in the Stage 2b → Stage 3 composition.
//!
//! Synthetic store with controlled redundancy. Asserts that
//! `consolidate_gac` (mutation path) supersedes the expected count
//! of non-representatives, and that
//! `vector_index_rebuild_with_opts(current_only=true)` produces an
//! HNSW sidecar sized to the representative set instead of the full
//! fact list.
//!
//! Marked `#[ignore]` because the default `model2vec` provider
//! downloads `potion-multilingual-128M` from HuggingFace on first
//! run. Run with:
//!
//! ```text
//! cargo test -p aidememo-core --features semantic --test gac_pipeline -- --ignored
//! ```

#![cfg(feature = "semantic")]

use aidememo_core::types::{FactType, GacOpts, VectorRebuildOpts};
use aidememo_core::{AideMemo, Config, FactInput};
use tempfile::tempdir;

fn open_temp_wiki(dir: &tempfile::TempDir) -> AideMemo {
    let mut config = Config::default();
    if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
        config.store.backend = "redb".to_string();
    }
    let suffix = if config.store.backend == "redb" {
        "redb"
    } else {
        "sqlite"
    };
    let store_path = dir.path().join(format!("test.{suffix}"));
    config.store.path = store_path.to_string_lossy().into_owned();
    AideMemo::open(&store_path, config).unwrap()
}

fn add(wiki: &AideMemo, content: &str) {
    wiki.fact_add(FactInput {
        content: content.into(),
        ..Default::default()
    })
    .expect("fact_add");
}

fn add_typed(wiki: &AideMemo, content: &str, ty: FactType) {
    wiki.fact_add(FactInput {
        content: content.into(),
        fact_type: Some(ty),
        ..Default::default()
    })
    .expect("fact_add");
}

fn seed_store(wiki: &AideMemo) {
    // Tight cluster — three near-paraphrases of the same Redis claim.
    // Expected to land at d̄ < 1-θ for θ=0.85.
    add(
        wiki,
        "Redis is an in-memory key-value store used as a cache",
    );
    add(wiki, "Redis is an in-memory key-value cache");
    add(
        wiki,
        "Redis serves as an in-memory cache and key-value store",
    );

    // Distinct singletons — different topics, should not cluster
    // with each other or with the Redis facts.
    add(wiki, "Postgres uses MVCC for transaction isolation");
    add(wiki, "Kubernetes schedules pods onto worker nodes");
    add(
        wiki,
        "Rust's borrow checker enforces aliasing rules at compile time",
    );
    add(
        wiki,
        "GraphQL exposes a typed query interface over a single endpoint",
    );
    add(wiki, "Kafka partitions topics for horizontal scaling");
}

#[test]
#[ignore = "downloads model2vec weights from HuggingFace — local only"]
fn gac_pipeline_supersede_then_current_only_rebuild_shrinks_index() {
    let dir = tempdir().unwrap();
    let wiki = open_temp_wiki(&dir);

    seed_store(&wiki);
    let total = wiki.stats().unwrap().fact_count;
    assert_eq!(total, 8, "seed produced unexpected fact count");

    // Stage 1 — dry-run reports clusters but mutates nothing.
    let dry = wiki
        .consolidate_gac(GacOpts {
            theta: 0.85,
            dry_run: true,
            spread_residual_budget: 0,
            use_cold_tier: false,
            protected_types: vec![],
        })
        .expect("consolidate_gac dry-run");
    assert_eq!(dry.facts_processed, 8);
    assert!(
        dry.tight_clusters >= 1,
        "expected at least one tight cluster from the Redis paraphrases, got {}",
        dry.tight_clusters
    );
    assert_eq!(dry.tight_collapsed, 0, "dry-run must not mutate");
    assert_eq!(dry.spread_archived, 0, "dry-run must not mutate");
    assert_eq!(
        wiki.stats().unwrap().fact_count,
        8,
        "dry-run must leave the store untouched"
    );

    // Stage 2b — apply with default (supersede). Non-representatives
    // get superseded but stay in the store.
    let applied = wiki
        .consolidate_gac(GacOpts {
            theta: 0.85,
            dry_run: false,
            spread_residual_budget: 0,
            use_cold_tier: false,
            protected_types: vec![],
        })
        .expect("consolidate_gac apply");
    let expected_collapsed = applied.tight_collapsed + applied.spread_archived;
    assert!(
        expected_collapsed >= 2,
        "expected at least 2 non-reps to be superseded (3-fact tight cluster), got {}",
        expected_collapsed
    );
    assert_eq!(
        applied.archived_to_cold, 0,
        "supersede mode must not move facts to cold-tier"
    );

    // Total fact count unchanged — supersede flips a flag, doesn't delete.
    let stats_after = wiki.stats().unwrap();
    assert_eq!(
        stats_after.fact_count, 8,
        "supersede must preserve raw fact count"
    );

    // Stage 3 — default rebuild keeps every fact in HNSW (current
    // contract preserves `as_of` historical retrieval).
    let full = wiki
        .vector_index_rebuild_with_opts(VectorRebuildOpts::default())
        .expect("vector_index_rebuild full");
    assert_eq!(full.facts_indexed, 8, "default rebuild indexes all facts");
    assert_eq!(full.superseded_skipped, 0);

    // Stage 3 — with current_only, the HNSW excludes superseded facts.
    let current_only = wiki
        .vector_index_rebuild_with_opts(VectorRebuildOpts { current_only: true })
        .expect("vector_index_rebuild current_only");
    assert_eq!(
        current_only.superseded_skipped, expected_collapsed,
        "current_only should skip exactly the facts consolidate just superseded"
    );
    assert_eq!(
        current_only.facts_indexed,
        8 - expected_collapsed,
        "indexed count must equal representative set size"
    );

    // Re-running consolidate_gac is idempotent: representatives are
    // already canonical, so a second pass collapses nothing.
    let second = wiki
        .consolidate_gac(GacOpts {
            theta: 0.85,
            dry_run: false,
            spread_residual_budget: 0,
            use_cold_tier: false,
            protected_types: vec![],
        })
        .expect("consolidate_gac second pass");
    assert_eq!(
        second.tight_collapsed + second.spread_archived,
        0,
        "second consolidate pass must be a no-op"
    );
}

#[test]
#[ignore = "downloads model2vec weights from HuggingFace — local only"]
fn protected_types_pass_through_gac_unchanged() {
    // Mirror the production pattern: classified Preference facts +
    // unclassified Notes mixed into one store. After consolidate
    // with protected_types=[Preference, Lesson, Error], the
    // personalisation tier should be untouched even when its
    // members would otherwise form a tight cluster.
    let dir = tempdir().unwrap();
    let wiki = open_temp_wiki(&dir);

    // 3 Preference paraphrases — would absolutely cluster tight at
    // θ=0.85 if not protected.
    add_typed(
        &wiki,
        "I prefer dark mode in my editor",
        FactType::Preference,
    );
    add_typed(
        &wiki,
        "I like dark theme for my editor",
        FactType::Preference,
    );
    add_typed(
        &wiki,
        "Dark theme is my preference for the editor",
        FactType::Preference,
    );

    // 3 Note paraphrases — these SHOULD cluster + collapse.
    add(&wiki, "Redis is an in-memory key-value cache");
    add(&wiki, "Redis is an in-memory key-value store used as cache");
    add(&wiki, "Redis serves as an in-memory cache and KV store");

    // 2 Note singletons — distinct topics, no cluster.
    add(&wiki, "Postgres uses MVCC for transaction isolation");
    add(&wiki, "Kubernetes schedules pods onto worker nodes");

    let stats = wiki
        .consolidate_gac(GacOpts {
            theta: 0.85,
            dry_run: false,
            spread_residual_budget: 0,
            use_cold_tier: false,
            protected_types: vec![FactType::Preference, FactType::Lesson, FactType::Error],
        })
        .expect("consolidate_gac with protect");

    assert_eq!(
        stats.protected_skipped, 3,
        "all 3 Preference facts must be excluded from clustering"
    );
    assert_eq!(
        stats.facts_processed, 5,
        "remaining 5 Note facts go through clustering"
    );
    // The Note paraphrase cluster shape is embedding-model-
    // dependent (model2vec at θ=0.85 sometimes lands one Redis
    // restatement just below the threshold). The load-bearing
    // assertion is "Preferences are protected" — we only sanity-
    // check that *something* in the unprotected pool collapsed.
    let collapsed = stats.tight_collapsed + stats.spread_archived;
    assert!(
        collapsed >= 1,
        "at least one of the 3 unprotected Note paraphrases should \
         collapse; got {} (cluster shape is model-dependent — bump \
         to a stronger model if this regresses)",
        collapsed
    );

    // Total fact_count is preserved (supersede flips a flag), but
    // all 3 Preferences must remain CURRENT — that's the lever the
    // operator is paying for.
    let pref_facts: Vec<_> = wiki
        .fact_list(aidememo_core::FactListOpts {
            fact_type: Some(FactType::Preference),
            current_only: true,
            ..Default::default()
        })
        .expect("fact_list");
    assert_eq!(
        pref_facts.len(),
        3,
        "all 3 Preferences must survive as current, none superseded"
    );
}
