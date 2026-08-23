//! Offline fiscal sign-later queue.
//!
//! When the configured signer is briefly unreachable, the till still completes the
//! sale and writes the unsigned transaction here. A background sweeper retries it
//! until the authority accepts it, or until a permanent rejection moves it to the
//! dead-letter state for an operator.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FiscalQueueStatus {
    Pending,
    Signed,
    DeadLetter,
}

impl FiscalQueueStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Signed => "signed",
            Self::DeadLetter => "dead_letter",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "signed" => Self::Signed,
            "dead_letter" => Self::DeadLetter,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewFiscalQueueEntry {
    pub subject_kind: String,
    pub subject_id: Uuid,
    pub provider: String,
    pub payload_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FiscalQueueRow {
    pub id: Uuid,
    pub subject_kind: String,
    pub subject_id: Uuid,
    pub provider: String,
    pub payload_json: String,
    pub status: FiscalQueueStatus,
    pub attempts: i64,
    pub next_attempt_at: DateTime<Utc>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct FiscalReceiptUpdate {
    pub provider: String,
    pub fiscal_id: Option<String>,
    pub signature: Option<String>,
    pub qr_payload: Option<String>,
    pub signed_at: Option<DateTime<Utc>>,
    pub pending: bool,
}

fn parse_uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap_or_else(|_| Uuid::nil())
}

fn parse_timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_queue(row: sqlx::sqlite::SqliteRow) -> FiscalQueueRow {
    let next_attempt_at: String = row.try_get("next_attempt_at").unwrap_or_default();
    let created_at: String = row.try_get("created_at").unwrap_or_default();
    let updated_at: String = row.try_get("updated_at").unwrap_or_default();
    let status: String = row.try_get("status").unwrap_or_default();
    FiscalQueueRow {
        id: parse_uuid(&row.try_get::<String, _>("id").unwrap_or_default()),
        subject_kind: row.try_get("subject_kind").unwrap_or_default(),
        subject_id: parse_uuid(&row.try_get::<String, _>("subject_id").unwrap_or_default()),
        provider: row.try_get("provider").unwrap_or_default(),
        payload_json: row.try_get("payload_json").unwrap_or_default(),
        status: FiscalQueueStatus::parse(&status),
        attempts: row.try_get("attempts").unwrap_or_default(),
        next_attempt_at: parse_timestamp(&next_attempt_at),
        last_error: row
            .try_get::<Option<String>, _>("last_error")
            .unwrap_or(None),
        created_at: parse_timestamp(&created_at),
        updated_at: parse_timestamp(&updated_at),
    }
}

/// Enqueue a transaction for later signing. Re-enqueueing the same subject is a no-op
/// that returns the existing row, so a retried finalize cannot create two sign jobs.
pub async fn enqueue_fiscal_signing(
    pool: &SqlitePool,
    entry: NewFiscalQueueEntry,
) -> Result<FiscalQueueRow, PoolError> {
    if let Some(existing) =
        fetch_fiscal_queue_for_subject(pool, &entry.subject_kind, entry.subject_id).await?
    {
        return Ok(existing);
    }

    let id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO fiscal_signing_queue \
         (id, subject_kind, subject_id, provider, payload_json, status, attempts, next_attempt_at, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, 'pending', 0, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(&entry.subject_kind)
    .bind(entry.subject_id.to_string())
    .bind(&entry.provider)
    .bind(&entry.payload_json)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;

    fetch_fiscal_queue_for_subject(pool, &entry.subject_kind, entry.subject_id)
        .await?
        .ok_or_else(|| PoolError::Other("fiscal queue insert vanished".into()))
}

pub async fn fetch_fiscal_queue_for_subject(
    pool: &SqlitePool,
    subject_kind: &str,
    subject_id: Uuid,
) -> Result<Option<FiscalQueueRow>, PoolError> {
    let row = sqlx::query(
        "SELECT id, subject_kind, subject_id, provider, payload_json, status, attempts, \
         next_attempt_at, last_error, created_at, updated_at \
         FROM fiscal_signing_queue WHERE subject_kind = ? AND subject_id = ?",
    )
    .bind(subject_kind)
    .bind(subject_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(row_to_queue))
}

pub async fn fetch_due_fiscal_signings(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<FiscalQueueRow>, PoolError> {
    let now = Utc::now().to_rfc3339();
    let rows = sqlx::query(
        "SELECT id, subject_kind, subject_id, provider, payload_json, status, attempts, \
         next_attempt_at, last_error, created_at, updated_at \
         FROM fiscal_signing_queue \
         WHERE status = 'pending' AND next_attempt_at <= ? \
         ORDER BY next_attempt_at ASC LIMIT ?",
    )
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_queue).collect())
}

pub async fn schedule_fiscal_retry(
    pool: &SqlitePool,
    id: Uuid,
    delay_seconds: i64,
    last_error: &str,
) -> Result<(), PoolError> {
    let now = Utc::now();
    let next = now + chrono::Duration::seconds(delay_seconds.max(1));
    sqlx::query(
        "UPDATE fiscal_signing_queue \
         SET status = 'pending', attempts = attempts + 1, next_attempt_at = ?, last_error = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(next.to_rfc3339())
    .bind(last_error)
    .bind(now.to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_fiscal_signed(pool: &SqlitePool, id: Uuid) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE fiscal_signing_queue \
         SET status = 'signed', last_error = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(&now)
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_fiscal_dead_letter(
    pool: &SqlitePool,
    id: Uuid,
    last_error: &str,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE fiscal_signing_queue \
         SET status = 'dead_letter', attempts = attempts + 1, last_error = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(last_error)
    .bind(&now)
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn count_fiscal_queue(
    pool: &SqlitePool,
    status: FiscalQueueStatus,
) -> Result<i64, PoolError> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM fiscal_signing_queue WHERE status = ?")
            .bind(status.as_str())
            .fetch_one(pool)
            .await?;
    Ok(count)
}

pub async fn apply_order_fiscal_receipt(
    pool: &SqlitePool,
    order_id: Uuid,
    receipt: &FiscalReceiptUpdate,
) -> Result<(), PoolError> {
    sqlx::query(
        "UPDATE orders SET fiscal_provider = ?, fiscal_id = ?, fiscal_signature = ?, \
         fiscal_qr_payload = ?, fiscal_signed_at = ?, fiscal_pending = ? WHERE id = ?",
    )
    .bind(&receipt.provider)
    .bind(&receipt.fiscal_id)
    .bind(&receipt.signature)
    .bind(&receipt.qr_payload)
    .bind(receipt.signed_at.map(|ts| ts.to_rfc3339()))
    .bind(i64::from(receipt.pending))
    .bind(order_id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn apply_return_fiscal_receipt(
    pool: &SqlitePool,
    return_id: Uuid,
    receipt: &FiscalReceiptUpdate,
) -> Result<(), PoolError> {
    sqlx::query(
        "UPDATE returns SET fiscal_provider = ?, fiscal_id = ?, fiscal_signature = ?, \
         fiscal_qr_payload = ?, fiscal_signed_at = ?, fiscal_pending = ? WHERE id = ?",
    )
    .bind(&receipt.provider)
    .bind(&receipt.fiscal_id)
    .bind(&receipt.signature)
    .bind(&receipt.qr_payload)
    .bind(receipt.signed_at.map(|ts| ts.to_rfc3339()))
    .bind(i64::from(receipt.pending))
    .bind(return_id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}
