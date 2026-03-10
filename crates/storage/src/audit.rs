//! Audit log for traceability (order finalization, HQ submission, etc.).

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

pub async fn record(
    pool: &SqlitePool,
    event_type: &str,
    entity_id: Option<Uuid>,
    payload: &str,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO audit_log (event_type, entity_id, payload, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(event_type)
    .bind(entity_id.map(|u| u.to_string()))
    .bind(payload)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}
