//! Timestamps and run ids (spec 8.4: "run-id is a timestamp + short random
//! suffix").
//!
//! No timezone library is pinned, so timestamps are rendered in UTC (RFC
//! 3339 with a `Z` suffix) via the civil-from-days algorithm; this is
//! recorded in docs/DECISIONS.md (I-27).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch (0 for times before 1970 or clock
/// errors, which cannot happen on supported systems).
#[must_use]
pub fn unix_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// `SystemTime::now()` in milliseconds since the epoch.
#[must_use]
pub fn now_millis() -> u64 {
    unix_millis(SystemTime::now())
}

/// Days since 1970-01-01 to (year, month, day) by Howard Hinnant's
/// `civil_from_days` algorithm (`chrono`-free).
#[must_use]
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// RFC 3339 UTC timestamp with millisecond precision, e.g.
/// `2026-10-16T18:55:12.345Z` (report `started_at`).
#[must_use]
pub fn rfc3339_utc(millis: u64) -> String {
    let secs = millis / 1_000;
    let ms = millis % 1_000;
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = rest / 3_600;
    let minute = (rest % 3_600) / 60;
    let second = rest % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{ms:03}Z")
}

/// `YYYYMMDD-HHMMSS` in UTC (the timestamp half of a run id).
#[must_use]
pub fn compact_utc(millis: u64) -> String {
    let secs = millis / 1_000;
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = rest / 3_600;
    let minute = (rest % 3_600) / 60;
    let second = rest % 60;
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

/// FNV-1a 64-bit (used for the run-id suffix and screenshot-hash caching).
#[must_use]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A run id: `YYYYMMDD-HHMMSS-<4 hex>` (Appendix B), where the suffix mixes
/// the clock, the process id and a per-process counter.
#[must_use]
pub fn run_id(now: SystemTime) -> String {
    let millis = unix_millis(now);
    let counter = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut seed = Vec::with_capacity(24);
    seed.extend_from_slice(&millis.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(&counter.to_le_bytes());
    let suffix = format!("{:04x}", fnv1a(&seed) & 0xffff);
    format!("{}-{suffix}", compact_utc(millis))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn civil_algorithm_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(
            civil_from_days(11_016),
            (2000, 2, 29),
            "2000 is a leap year"
        );
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(19_783), (2024, 3, 1));
    }

    #[test]
    fn rfc3339_renders_utc() {
        let millis = unix_millis(UNIX_EPOCH + Duration::from_millis(1_234_567_890_123));
        assert_eq!(rfc3339_utc(millis), "2009-02-13T23:31:30.123Z");
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn compact_and_run_id_shape() {
        let millis = unix_millis(UNIX_EPOCH + Duration::from_millis(1_234_567_890_123));
        assert_eq!(compact_utc(millis), "20090213-233130");
        let time = UNIX_EPOCH + Duration::from_millis(1_234_567_890_123);
        let id = run_id(time);
        assert_eq!(id.len(), "20090213-233130-cccc".len());
        assert!(id.starts_with("20090213-233130-"), "{id}");
        assert!(id["20090213-233130-".len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        // A second call never collides even within the same millisecond.
        assert_ne!(run_id(time), id);
    }

    #[test]
    fn fnv1a_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
