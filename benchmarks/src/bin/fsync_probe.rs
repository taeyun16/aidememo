//! Measures the cost of redb's per-commit fsync on the host machine.
//!
//! `fact_add` p95 sits at ~5 ms across every wiki size in the perf
//! bench, which strongly suggests the cost is the macOS APFS fsync
//! per redb commit (Durability::Immediate, default). This bin
//! confirms it by inserting the same payload N times into a fresh
//! redb file under each durability level and printing p50 / p95.
//!
//! No WikiGraph involvement — we want the rawest possible measurement
//! of the storage layer.
//!
//! Run:
//!   cargo run --release --bin fsync_probe

use std::path::Path;
use std::time::Instant;

use redb::{Database, Durability, TableDefinition};
use tempfile::TempDir;

const TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("probe");
const N: usize = 100;
const PAYLOAD_BYTES: usize = 256;

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn run(label: &str, db: &Database, durability: Durability) {
    let payload = vec![0u8; PAYLOAD_BYTES];
    let mut times = Vec::with_capacity(N);
    for i in 0..N {
        let key = (i as u64).to_be_bytes();
        let t0 = Instant::now();
        let mut txn = db.begin_write().expect("begin_write");
        txn.set_durability(durability);
        {
            let mut tbl = txn.open_table(TABLE).expect("open_table");
            tbl.insert(&key as &[u8], payload.as_slice())
                .expect("insert");
        }
        txn.commit().expect("commit");
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    println!(
        "{:<14}  p50={:>7.3} ms   p95={:>7.3} ms   p99={:>7.3} ms   mean={:>7.3} ms",
        label,
        percentile(&times, 0.50),
        percentile(&times, 0.95),
        percentile(&times, 0.99),
        mean,
    );
}

fn main() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("probe.redb");

    println!(
        "=== redb commit-latency probe ({} writes, {} B payload) ===",
        N, PAYLOAD_BYTES
    );
    println!("file: {}", path.display());

    {
        let db = Database::create(&path).expect("create");
        run("Immediate", &db, Durability::Immediate); // default; fsyncs
    }
    {
        let dir2 = TempDir::new().expect("tempdir");
        let path2 = dir2.path().join("probe.redb");
        let db = Database::create(&path2).expect("create");
        run("Eventual", &db, Durability::Eventual); // queues, no per-commit fsync
    }
    {
        let dir3 = TempDir::new().expect("tempdir");
        let path3 = dir3.path().join("probe.redb");
        let db = Database::create(&path3).expect("create");
        run("None", &db, Durability::None); // no flush at all
    }

    let _ = Path::new(&path); // silence unused warning under future paths
}
