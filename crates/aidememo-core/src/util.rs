//! Common utility functions used across aidememo-core.

use std::time::{SystemTime, UNIX_EPOCH};

/// Get current Unix timestamp in milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

/// Compute SHA-256 hex digest of a string.
pub fn sha256_hex_str(s: &str) -> String {
    sha256_hex_bytes(s.as_bytes())
}

/// Compute SHA-256 hex digest of bytes.
pub fn sha256_hex_bytes(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}
