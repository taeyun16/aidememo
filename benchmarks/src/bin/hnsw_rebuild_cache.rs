//! Measure the incremental embedding cache speedup on
//! `vector_index_rebuild`.
//!
//! Calls rebuild once with a cold sidecar (full re-embed) and a
//! second time with the in-memory + on-disk cache populated. The
//! delta is the lower bound on the win operators see when they
//! call `wg ingest` on a corpus where most facts are unchanged.

use std::path::Path;
use std::time::Instant;
use wg_core::WikiGraph;

fn main() {
    let store_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/wg-bench-miracl/_meta/wiki.redb".to_string());
    println!("store: {store_path}");

    let config = wg_core::Config::default();
    let wiki = WikiGraph::open(Path::new(&store_path), config).expect("open");

    // Drop any existing sidecar so the first rebuild is genuinely cold.
    let sidecar = Path::new(&store_path).with_extension("hnsw.bin");
    let _ = std::fs::remove_file(&sidecar);

    let t = Instant::now();
    let n = wiki.vector_index_rebuild().expect("cold rebuild");
    let cold = t.elapsed();
    println!("  cold (no cache):     {cold:>8.2?}  n={n}");

    let t = Instant::now();
    let n = wiki.vector_index_rebuild().expect("warm rebuild");
    let warm_inmem = t.elapsed();
    println!("  warm (in-mem cache): {warm_inmem:>8.2?}  n={n}");

    // Force the cache to come from disk by re-opening the store.
    drop(wiki);
    let config = wg_core::Config::default();
    let wiki = WikiGraph::open(Path::new(&store_path), config).expect("reopen");

    let t = Instant::now();
    let n = wiki.vector_index_rebuild().expect("disk-warm rebuild");
    let warm_disk = t.elapsed();
    println!("  warm (disk cache):   {warm_disk:>8.2?}  n={n}");

    let saved_inmem = cold.as_secs_f64() - warm_inmem.as_secs_f64();
    let saved_disk = cold.as_secs_f64() - warm_disk.as_secs_f64();
    println!(
        "\n  speedup (in-mem):    -{:.2}s  ({:.0}% faster)",
        saved_inmem,
        100.0 * saved_inmem / cold.as_secs_f64()
    );
    println!(
        "  speedup (disk):      -{:.2}s  ({:.0}% faster)",
        saved_disk,
        100.0 * saved_disk / cold.as_secs_f64()
    );
}
