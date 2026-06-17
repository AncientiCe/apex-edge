//! Pure freshness / business-continuity math.
//!
//! HQ syncs periodically. When the WAN/HQ is unreachable the edge keeps selling, but
//! operators need to know how stale the synced baselines are. This module derives
//! "sync staleness" and a bounded "degraded mode" flag from the last successful sync
//! time, independent of storage or clocks (the caller passes `now`).

use chrono::{DateTime, Utc};

/// Default seconds of sync staleness after which the hub is considered degraded.
/// Roughly 3x the nominal ~300s sync cadence; override via config.
pub const DEFAULT_DEGRADED_THRESHOLD_SECONDS: i64 = 900;

/// Freshness assessment derived from the last successful sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncFreshness {
    /// Seconds since the last successful sync; `None` when no sync has ever succeeded.
    pub staleness_seconds: Option<i64>,
    /// True when staleness exceeds the degraded threshold, or no sync has happened.
    pub degraded: bool,
}

/// Compute staleness (in seconds) since the last successful sync.
///
/// Returns `None` when there has been no successful sync. A `last_success` in the
/// future (clock skew) clamps to zero rather than going negative.
pub fn staleness_seconds(last_success: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<i64> {
    last_success.map(|t| (now - t).num_seconds().max(0))
}

/// Assess sync freshness against a degraded threshold.
///
/// If no sync has ever succeeded the hub is degraded (no trustworthy baseline).
pub fn assess_freshness(
    last_success: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    threshold_seconds: i64,
) -> SyncFreshness {
    let staleness = staleness_seconds(last_success, now);
    let degraded = match staleness {
        None => true,
        Some(s) => s > threshold_seconds,
    };
    SyncFreshness {
        staleness_seconds: staleness,
        degraded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-17T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn staleness_is_none_without_a_sync() {
        assert_eq!(staleness_seconds(None, now()), None);
    }

    #[test]
    fn staleness_counts_seconds_since_last_sync() {
        let last = now() - Duration::seconds(120);
        assert_eq!(staleness_seconds(Some(last), now()), Some(120));
    }

    #[test]
    fn future_sync_time_clamps_to_zero() {
        let last = now() + Duration::seconds(30);
        assert_eq!(staleness_seconds(Some(last), now()), Some(0));
    }

    #[test]
    fn fresh_sync_is_not_degraded() {
        let last = now() - Duration::seconds(60);
        let f = assess_freshness(Some(last), now(), DEFAULT_DEGRADED_THRESHOLD_SECONDS);
        assert_eq!(f.staleness_seconds, Some(60));
        assert!(!f.degraded);
    }

    #[test]
    fn stale_sync_is_degraded() {
        let last = now() - Duration::seconds(DEFAULT_DEGRADED_THRESHOLD_SECONDS + 1);
        let f = assess_freshness(Some(last), now(), DEFAULT_DEGRADED_THRESHOLD_SECONDS);
        assert!(f.degraded);
    }

    #[test]
    fn never_synced_is_degraded() {
        let f = assess_freshness(None, now(), DEFAULT_DEGRADED_THRESHOLD_SECONDS);
        assert!(f.degraded);
        assert_eq!(f.staleness_seconds, None);
    }

    #[test]
    fn exactly_at_threshold_is_not_degraded() {
        let last = now() - Duration::seconds(DEFAULT_DEGRADED_THRESHOLD_SECONDS);
        let f = assess_freshness(Some(last), now(), DEFAULT_DEGRADED_THRESHOLD_SECONDS);
        assert!(!f.degraded);
    }
}
