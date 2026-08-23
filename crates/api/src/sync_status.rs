//! Sync status endpoint: last sync, run state, per-entity progress.

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::pos::AppState;
use apex_edge_domain::continuity::{assess_freshness, DEFAULT_DEGRADED_THRESHOLD_SECONDS};
use apex_edge_metrics::{EDGE_DEGRADED_MODE, SYNC_STALENESS_SECONDS};
use apex_edge_storage::{
    get_entity_sync_statuses, get_last_successful_sync_at, get_latest_sync_run,
};

#[derive(Serialize)]
pub struct SyncStatusResponse {
    pub last_sync_at: Option<DateTime<Utc>>,
    pub is_syncing: bool,
    /// Seconds since the last *successful* sync; `null` when none has ever succeeded.
    pub sync_staleness_seconds: Option<i64>,
    /// True when the hub is running on stale baselines (continuity/degraded mode).
    pub degraded: bool,
    pub entities: Vec<EntitySyncStatusDto>,
}

/// Degraded-mode threshold (seconds), overridable via env for tuning per deployment.
fn degraded_threshold_seconds() -> i64 {
    std::env::var("APEX_EDGE_SYNC_STALENESS_DEGRADED_SECONDS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_DEGRADED_THRESHOLD_SECONDS)
}

#[derive(Serialize)]
pub struct EntitySyncStatusDto {
    pub entity: String,
    pub current: u64,
    pub total: Option<u64>,
    pub percent: Option<f64>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub status: String,
}

/// GET /sync/status: latest run and per-entity progress for the status page.
pub async fn sync_status(
    State(state): State<AppState>,
) -> Result<Json<SyncStatusResponse>, axum::http::StatusCode> {
    let run = get_latest_sync_run(&state.pool)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let entities = get_entity_sync_statuses(&state.pool)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    let last_sync_at = run.as_ref().and_then(|r| r.finished_at.or(r.started_at));
    let is_syncing = run.as_ref().map(|r| r.state == "running").unwrap_or(false);

    // Freshness / degraded mode from the durable last-success marker.
    let last_success = get_last_successful_sync_at(&state.pool)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let freshness = assess_freshness(last_success, Utc::now(), degraded_threshold_seconds());
    metrics::gauge!(SYNC_STALENESS_SECONDS).set(freshness.staleness_seconds.unwrap_or(0) as f64);
    metrics::gauge!(EDGE_DEGRADED_MODE).set(if freshness.degraded { 1.0 } else { 0.0 });

    let entities = entities
        .into_iter()
        .map(|e| EntitySyncStatusDto {
            entity: e.entity,
            current: e.current,
            total: e.total,
            percent: e.percent,
            last_synced_at: e.updated_at,
            status: e.status,
        })
        .collect();

    Ok(Json(SyncStatusResponse {
        last_sync_at,
        is_syncing,
        sync_staleness_seconds: freshness.staleness_seconds,
        degraded: freshness.degraded,
        entities,
    }))
}
