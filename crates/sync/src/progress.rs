//! Sync progress: per-entity and overall %. Usage is allowed at any progress (partial sync is valid).
//! The app can operate with 0%, 50%, or 100% sync; progress is informational only.

use serde::{Deserialize, Serialize};

/// Per-entity sync progress (current sequence and optional total for %).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEntityProgress {
    pub entity: String,
    /// Current checkpoint / sequence (number of items ingested so far).
    pub current: u64,
    /// Total items available from source, if known (enables percent).
    pub total: Option<u64>,
}

/// Overall sync progress summary; overall_percent is set only when all entities have total.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProgressSummary {
    pub entities: Vec<SyncEntityProgress>,
    /// 0.0..=100.0 when all entities have total; None if any entity has unknown total.
    pub overall_percent: Option<f64>,
}

impl SyncEntityProgress {
    /// Progress as 0.0..=100.0 when total is known; None otherwise.
    pub fn percent(&self) -> Option<f64> {
        self.total.and_then(|t| {
            if t == 0 {
                Some(100.0)
            } else {
                Some((self.current as f64 / t as f64).min(1.0) * 100.0)
            }
        })
    }
}

impl SyncProgressSummary {
    /// Build summary from per-entity progress; compute overall % when all have total.
    pub fn from_entities(entities: Vec<SyncEntityProgress>) -> Self {
        let (sum_current, sum_total) = entities.iter().fold((0u64, 0u64), |(c, t), e| {
            (c + e.current, t + e.total.unwrap_or(0))
        });
        let all_have_total = entities.iter().all(|e| e.total.is_some());
        let overall_percent = if all_have_total && sum_total > 0 {
            Some((sum_current as f64 / sum_total as f64).min(1.0) * 100.0)
        } else {
            None
        };
        Self {
            entities,
            overall_percent,
        }
    }

    /// Usage is allowed at any progress (partial sync is valid).
    pub fn is_complete(&self) -> bool {
        self.overall_percent.map(|p| p >= 100.0).unwrap_or(false)
    }
}
