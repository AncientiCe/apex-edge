//! Config and sync checkpoints.

use chrono::Utc;
use sqlx::SqlitePool;

use crate::pool::PoolError;

pub async fn get_sync_checkpoint(
    pool: &SqlitePool,
    entity: &str,
) -> Result<Option<i64>, PoolError> {
    let row =
        sqlx::query_scalar::<_, i64>("SELECT sequence FROM sync_checkpoints WHERE entity = ?")
            .bind(entity)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

pub async fn set_sync_checkpoint(
    pool: &SqlitePool,
    entity: &str,
    sequence: i64,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO sync_checkpoints (entity, sequence, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(entity) DO UPDATE SET sequence = ?, updated_at = ?",
    )
    .bind(entity)
    .bind(sequence)
    .bind(&now)
    .bind(sequence)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}
