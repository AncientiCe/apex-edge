//! Sync status endpoint: last sync, run state, per-entity progress.

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::pos::AppState;
use apex_edge_storage::{get_entity_sync_statuses, get_latest_sync_run};

#[derive(Serialize)]
pub struct SyncStatusResponse {
    pub last_sync_at: Option<DateTime<Utc>>,
    pub is_syncing: bool,
    pub entities: Vec<EntitySyncStatusDto>,
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
        entities,
    }))
}
