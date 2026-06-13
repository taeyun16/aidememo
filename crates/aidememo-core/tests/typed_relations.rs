//! Integration test: ingest the markdown corpus under
//! `tests/fixtures/typed-relations/` and assert that the typed-relation
//! extractor produced the expected edges (instead of the catch-all
//! `references` fallback).

use aidememo_core::{AideMemo, Config, TraverseDirection};
use std::collections::HashSet;
use std::path::PathBuf;
use tempfile::tempdir;

fn fixtures_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/typed-relations");
    p
}

fn open_temp_wiki() -> (tempfile::TempDir, AideMemo) {
    let dir = tempdir().unwrap();
    let mut config = Config::default();
    if cfg!(all(feature = "redb", not(feature = "sqlite"))) {
        config.store.backend = "redb".to_string();
    }
    let store_path = dir.path().join(if config.store.backend == "redb" {
        "test.redb"
    } else {
        "test.sqlite"
    });
    let wiki = AideMemo::open(&store_path, config).unwrap();
    (dir, wiki)
}

fn relations_for(wiki: &AideMemo, entity: &str) -> Vec<(String, String, String)> {
    wiki.relations_get(entity, TraverseDirection::Forward)
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let src = wiki
                .entity_get_by_id(r.source_id)
                .map(|e| e.name)
                .unwrap_or_default();
            let tgt = wiki
                .entity_get_by_id(r.target_id)
                .map(|e| e.name)
                .unwrap_or_default();
            (src, r.relation_type.0, tgt)
        })
        .collect()
}

#[test]
fn typed_relations_extracted_from_real_markdown() {
    let (_dir, wiki) = open_temp_wiki();
    let stats = wiki.ingest(&fixtures_root(), false).unwrap();

    assert!(
        stats.files_scanned >= 3,
        "expected to scan all 3 fixture files"
    );
    assert!(
        stats.relations_added > 0,
        "expected typed relations to be added"
    );

    // Build a global set of (source, type, target) triples.
    let mut all: HashSet<(String, String, String)> = HashSet::new();
    let entities = wiki
        .entity_list(aidememo_core::ListOpts::default())
        .unwrap();
    for e in &entities {
        for r in relations_for(&wiki, &e.name) {
            all.insert(r);
        }
    }

    // Spot-check distinctive typed edges. Names are case-preserved per
    // the source markdown; comparison is case-insensitive to be robust
    // against any normalization the store does.
    let expectations: &[(&str, &str, &str)] = &[
        // people.md — one fact per line, so all extract cleanly
        ("Alice", "works_at", "Acme"),
        ("Alice", "reports_to", "Bob"),
        ("Bob", "founded", "Acme"),
        ("Bob", "manages", "Engineering"),
        ("Carol", "advises", "Acme"),
        ("Carol", "invested_in", "Beta Corp"),
        ("Alice", "attended", "All Hands 2026"),
        // redis.md
        ("Redis", "depends_on", "Linux"),
        ("Redis", "uses", "Memory"),
        ("Memcached", "alternative_to", "Redis"),
        ("Redis Cluster", "extends", "Redis"),
        ("Redis Sentinel", "owned_by", "Redis"),
        // decisions.md
        ("Plan B", "supersedes", "Plan A"),
        ("Migration Phase 1", "blocks", "Migration Phase 2"),
        ("Migration Phase 2", "depends_on", "Schema v3"),
        ("Service Auth", "part_of", "API Gateway"),
    ];

    let lower_set: HashSet<(String, String, String)> = all
        .iter()
        .map(|(s, t, g)| (s.to_lowercase(), t.clone(), g.to_lowercase()))
        .collect();

    for (src, ty, tgt) in expectations {
        let triple = (src.to_lowercase(), ty.to_string(), tgt.to_lowercase());
        if !lower_set.contains(&triple) {
            // Dump everything we did extract so the test failure is debuggable.
            let mut sorted: Vec<_> = all.iter().collect();
            sorted.sort();
            eprintln!("--- all {} relations ---", sorted.len());
            for (s, t, g) in &sorted {
                eprintln!("  {} -{}-> {}", s, t, g);
            }
            panic!("missing typed relation: {} -{}-> {}", src, ty, tgt);
        }
    }

    // Typed relations should make up a meaningful share of the graph. The
    // file's own entity still emits a catch-all `references` edge to every
    // wikilink in its body, so `references` will outnumber typed by design.
    // We just assert typed is non-trivial and the right shape exists.
    let ref_count = all.iter().filter(|(_, ty, _)| ty == "references").count();
    let typed_count = all.iter().filter(|(_, ty, _)| ty != "references").count();
    assert!(
        typed_count >= 15,
        "expected ≥15 typed relations on this corpus, got {} (refs={})",
        typed_count,
        ref_count
    );
}

#[test]
fn typed_relations_survive_reingest() {
    let (_dir, wiki) = open_temp_wiki();

    let _ = wiki.ingest(&fixtures_root(), false).unwrap();
    let alice_first = relations_for(&wiki, "Alice");

    // Re-ingest (incremental). Counts shouldn't double; same triples should remain.
    let _ = wiki.ingest(&fixtures_root(), true).unwrap();
    let alice_second = relations_for(&wiki, "Alice");

    let first_set: HashSet<_> = alice_first.into_iter().collect();
    let second_set: HashSet<_> = alice_second.into_iter().collect();
    assert_eq!(
        first_set, second_set,
        "re-ingest should yield identical typed relations"
    );
}
