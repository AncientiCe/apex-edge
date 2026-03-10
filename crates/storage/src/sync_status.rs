//! Latest-only sync run and per-entity status (for status page and progress).

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use thiserror::Error;

use crate::pool::PoolError;

#[derive(Error, Debug)]
pub enum SyncStatusError {
    #[error("pool: {0}")]
    Pool(#[from] PoolError),
}

impl From<sqlx::Error> for SyncStatusError {
    fn from(e: sqlx::Error) -> Self {
        Self::Pool(PoolError::from(e))
    }
}

/// Latest sync run state: idle, running, success, failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestSyncRun {
    pub state: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Per-entity sync status for display.
#[derive(Debug, Clone)]
pub struct EntitySyncStatus {
    pub entity: String,
    pub current: u64,
    pub total: Option<u64>,
    pub percent: Option<f64>,
    pub updated_at: Option<DateTime<Utc>>,
    pub status: String,
}

fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Get latest sync run (single row; None if never run).
pub async fn get_latest_sync_run(
    pool: &SqlitePool,
) -> Result<Option<LatestSyncRun>, SyncStatusError> {
    let row = sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>)>(
        "SELECT state, started_at, finished_at, last_error FROM sync_run WHERE id = 'latest'",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(state, started_at, finished_at, last_error)| LatestSyncRun {
            state,
            started_at: started_at.as_deref().and_then(parse_datetime),
            finished_at: finished_at.as_deref().and_then(parse_datetime),
            last_error,
        },
    ))
}

/// Upsert latest sync run (replace the single row).
pub async fn upsert_latest_sync_run(
    pool: &SqlitePool,
    state: &str,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    last_error: Option<&str>,
) -> Result<(), SyncStatusError> {
    let started = started_at.map(|t| t.to_rfc3339());
    let finished = finished_at.map(|t| t.to_rfc3339());
    sqlx::query(
        "INSERT INTO sync_run (id, state, started_at, finished_at, last_error) VALUES ('latest', ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET state = ?, started_at = ?, finished_at = ?, last_error = ?",
    )
    .bind(state)
    .bind(started.as_deref())
    .bind(finished.as_deref())
    .bind(last_error)
    .bind(state)
    .bind(started.as_deref())
    .bind(finished.as_deref())
    .bind(last_error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get all entity sync statuses (for status page).
pub async fn get_entity_sync_statuses(
    pool: &SqlitePool,
) -> Result<Vec<EntitySyncStatus>, SyncStatusError> {
    let rows = sqlx::query_as::<_, (String, i64, Option<i64>, Option<f64>, Option<String>, String)>(
        "SELECT entity, current, total, percent, updated_at, status FROM entity_sync_status ORDER BY entity",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(entity, current, total, percent, updated_at, status)| EntitySyncStatus {
                entity,
                current: current as u64,
                total: total.map(|t| t as u64),
                percent,
                updated_at: updated_at.as_deref().and_then(parse_datetime),
                status,
            },
        )
        .collect())
}

/// Upsert one entity's sync status (replace row for that entity).
pub async fn upsert_entity_sync_status(
    pool: &SqlitePool,
    entity: &str,
    current: u64,
    total: Option<u64>,
    percent: Option<f64>,
    updated_at: DateTime<Utc>,
    status: &str,
) -> Result<(), SyncStatusError> {
    let updated = updated_at.to_rfc3339();
    sqlx::query(
        "INSERT INTO entity_sync_status (entity, current, total, percent, updated_at, status) VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(entity) DO UPDATE SET current = ?, total = ?, percent = ?, updated_at = ?, status = ?",
    )
    .bind(entity)
    .bind(current as i64)
    .bind(total.map(|t| t as i64))
    .bind(percent)
    .bind(&updated)
    .bind(status)
    .bind(current as i64)
    .bind(total.map(|t| t as i64))
    .bind(percent)
    .bind(&updated)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(())
}
