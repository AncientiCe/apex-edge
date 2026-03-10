//! Config and sync checkpoints.

use apex_edge_metrics::{
    DB_OPERATIONS_TOTAL, DB_OPERATION_DURATION_SECONDS, DB_OUTCOME_ERROR, DB_OUTCOME_SUCCESS,
};
use chrono::Utc;
use sqlx::SqlitePool;
use std::time::Instant;

use crate::pool::PoolError;

pub async fn get_sync_checkpoint(
    pool: &SqlitePool,
    entity: &str,
) -> Result<Option<i64>, PoolError> {
    const OP: &str = "get_sync_checkpoint";
    let start = Instant::now();
    let result =
        sqlx::query_scalar::<_, i64>("SELECT sequence FROM sync_checkpoints WHERE entity = ?")
            .bind(entity)
            .fetch_optional(pool)
            .await;
    let outcome = if result.is_ok() {
        DB_OUTCOME_SUCCESS
    } else {
        DB_OUTCOME_ERROR
    };
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => outcome);
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    result.map_err(Into::into)
}

pub async fn set_sync_checkpoint(
    pool: &SqlitePool,
    entity: &str,
    sequence: i64,
) -> Result<(), PoolError> {
    const OP: &str = "set_sync_checkpoint";
    let start = Instant::now();
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        "INSERT INTO sync_checkpoints (entity, sequence, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(entity) DO UPDATE SET sequence = ?, updated_at = ?",
    )
    .bind(entity)
    .bind(sequence)
    .bind(&now)
    .bind(sequence)
    .bind(&now)
    .execute(pool)
    .await;
    let outcome = if result.is_ok() {
        DB_OUTCOME_SUCCESS
    } else {
        DB_OUTCOME_ERROR
    };
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => outcome);
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    result?;
    Ok(())
}
