//! Outbox table for reliable order submission.

use apex_edge_metrics::{
    DB_OPERATIONS_TOTAL, DB_OPERATION_DURATION_SECONDS, DB_OUTCOME_ERROR, DB_OUTCOME_SUCCESS,
};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::time::Instant;
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug)]
pub struct OutboxRow {
    pub id: Uuid,
    pub payload: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub next_retry_at: Option<DateTime<Utc>>,
    pub attempts: i32,
    pub error_message: Option<String>,
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_opt_datetime(s: Option<&str>) -> Option<DateTime<Utc>> {
    s.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

pub async fn insert_outbox(pool: &SqlitePool, id: Uuid, payload: &str) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO outbox (id, payload, status, created_at, attempts) VALUES (?, ?, 'pending', ?, 0)",
    )
    .bind(id.to_string())
    .bind(payload)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn fetch_pending_outbox(
    pool: &SqlitePool,
    limit: i32,
) -> Result<Vec<OutboxRow>, PoolError> {
    const OP: &str = "fetch_pending_outbox";
    let start = Instant::now();
    let now = Utc::now().to_rfc3339();
    let rows_result = sqlx::query_as::<_, (String, String, String, String, Option<String>, i32, Option<String>)>(
        "SELECT id, payload, status, created_at, next_retry_at, attempts, error_message FROM outbox WHERE status = 'pending' AND (next_retry_at IS NULL OR next_retry_at <= ?) ORDER BY created_at LIMIT ?",
    )
    .bind(&now)
    .bind(limit)
    .fetch_all(pool)
    .await;
    let outcome = if rows_result.is_ok() {
        DB_OUTCOME_SUCCESS
    } else {
        DB_OUTCOME_ERROR
    };
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => outcome);
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    let rows = rows_result?;
    Ok(rows
        .into_iter()
        .map(
            |(id, payload, status, created_at, next_retry_at, attempts, error_message)| OutboxRow {
                id: Uuid::parse_str(&id).unwrap_or_default(),
                payload,
                status,
                created_at: parse_datetime(&created_at),
                next_retry_at: next_retry_at
                    .as_deref()
                    .and_then(|s| parse_opt_datetime(Some(s))),
                attempts,
                error_message,
            },
        )
        .collect())
}

pub async fn mark_delivered(pool: &SqlitePool, id: Uuid) -> Result<(), PoolError> {
    const OP: &str = "mark_delivered";
    let start = Instant::now();
    let result = sqlx::query("UPDATE outbox SET status = 'delivered' WHERE id = ?")
        .bind(id.to_string())
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

pub async fn schedule_retry(
    pool: &SqlitePool,
    id: Uuid,
    next_at: DateTime<Utc>,
) -> Result<(), PoolError> {
    const OP: &str = "schedule_retry";
    let start = Instant::now();
    let next_at_str = next_at.to_rfc3339();
    let result =
        sqlx::query("UPDATE outbox SET next_retry_at = ?, attempts = attempts + 1 WHERE id = ?")
            .bind(&next_at_str)
            .bind(id.to_string())
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

pub async fn mark_dead_letter(pool: &SqlitePool, id: Uuid, reason: &str) -> Result<(), PoolError> {
    const OP: &str = "mark_dead_letter";
    let start = Instant::now();
    let result =
        sqlx::query("UPDATE outbox SET status = 'dead_letter', error_message = ? WHERE id = ?")
            .bind(reason)
            .bind(id.to_string())
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
