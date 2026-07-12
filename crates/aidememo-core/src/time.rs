//! Centralised wall-clock helpers.
//!
//! Every place aidememo-core needs "now in epoch milliseconds" should call
//! [`current_epoch_ms`] rather than `SystemTime::now()` directly. This
//! lets benchmarks and tests pin the clock via `AIDEMEMO_NOW_MS` for
//! bit-identical reproducibility:
//!
//! ```text
//! AIDEMEMO_NOW_MS=1735689600000 aidememo ingest …   # 2025-01-01T00:00:00Z
//! ```
//!
//! Without `AIDEMEMO_NOW_MS`, behaviour matches the old `SystemTime::now()`
//! path exactly — production callers see no change.

use std::time::{SystemTime, UNIX_EPOCH};

/// Return the current epoch in milliseconds, honouring the `AIDEMEMO_NOW_MS`
/// override when set. Used for `created_at` / `updated_at` stamps and
/// any time-decay computation that should be reproducible across runs.
pub fn current_epoch_ms() -> u64 {
    if let Ok(v) = std::env::var("AIDEMEMO_NOW_MS")
        && let Ok(n) = v.parse::<u64>()
    {
        return n;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
