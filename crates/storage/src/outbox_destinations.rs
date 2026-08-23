//! Per-destination delivery state for the outbox.
//!
//! A submission is one row in `outbox`, but it can be owed to several places: HQ, a
//! Peppol access point, KSeF, a webhook. Each of those succeeds, retries and fails on its
//! own schedule, so delivery state cannot live on the submission — it lives here, one row
//! per (submission, destination).
//!
//! The submission row keeps a summary status so that everything already reading `outbox`
//! keeps working: `pending` while anyone is still owed it, `delivered` once every
//! destination has taken it, `dead_letter` if any destination never will.

use apex_edge_metrics::{
    DB_OPERATIONS_TOTAL, DB_OPERATION_DURATION_SECONDS, DB_OUTCOME_ERROR, DB_OUTCOME_SUCCESS,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::time::Instant;
use uuid::Uuid;

use crate::pool::PoolError;

/// Where a single delivery stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Owed, and will be attempted when `next_attempt_at` passes.
    Pending,
    /// This destination has it. Terminal.
    Delivered,
    /// This destination refused it too many times. Terminal, and needs an operator.
    DeadLetter,
}

impl DeliveryState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeliveryState::Pending => "pending",
            DeliveryState::Delivered => "delivered",
            DeliveryState::DeadLetter => "dead_letter",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "delivered" => DeliveryState::Delivered,
            "dead_letter" => DeliveryState::DeadLetter,
            _ => DeliveryState::Pending,
        }
    }
}

/// A destination as configuration describes it.
#[derive(Debug, Clone)]
pub struct NewDestination {
    /// Stable operator-facing name, unique per hub: `hq`, `peppol`, `ksef`.
    pub code: String,
    /// What kind of thing this is, which decides how the dispatcher talks to it.
    pub kind: String,
    pub endpoint: Option<String>,
    /// Kind-specific settings: auth headers, payload filters, participant ids.
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationRow {
    pub id: Uuid,
    pub code: String,
    pub kind: String,
    pub endpoint: Option<String>,
    pub enabled: bool,
    pub config: Value,
    pub created_at: DateTime<Utc>,
}

/// A delivery that is due now, joined to everything needed to attempt it.
#[derive(Debug, Clone)]
pub struct DueDelivery {
    pub attempt_id: Uuid,
    pub outbox_id: Uuid,
    pub payload: String,
    pub destination_id: Uuid,
    pub destination_code: String,
    pub destination_kind: String,
    pub endpoint: Option<String>,
    pub config: Value,
    /// Attempts already made against this destination, not against the submission.
    pub attempts: i64,
    pub last_error: Option<String>,
}

/// One delivery's state, without the payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryAttemptRow {
    pub attempt_id: Uuid,
    pub outbox_id: Uuid,
    pub destination_id: Uuid,
    pub destination_code: String,
    pub state: DeliveryState,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

fn parse_uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap_or_else(|_| Uuid::nil())
}

fn parse_timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_config(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::Object(Default::default()))
}

/// Wraps a query in the standard database counter/timer pair.
async fn observed<T, F>(operation: &'static str, call: F) -> Result<T, PoolError>
where
    F: std::future::Future<Output = Result<T, sqlx::Error>>,
{
    let start = Instant::now();
    let result = call.await;
    let outcome = if result.is_ok() {
        DB_OUTCOME_SUCCESS
    } else {
        DB_OUTCOME_ERROR
    };
    metrics::counter!(DB_OPERATIONS_TOTAL, "operation" => operation, "outcome" => outcome)
        .increment(1);
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, "operation" => operation)
        .record(start.elapsed().as_secs_f64());
    Ok(result?)
}

const DESTINATION_COLUMNS: &str = "id, code, kind, endpoint, enabled, config_json, created_at";

fn row_to_destination(row: &sqlx::sqlite::SqliteRow) -> DestinationRow {
    let enabled: i64 = row.try_get("enabled").unwrap_or(1);
    DestinationRow {
        id: parse_uuid(&row.try_get::<String, _>("id").unwrap_or_default()),
        code: row.try_get("code").unwrap_or_default(),
        kind: row.try_get("kind").unwrap_or_default(),
        endpoint: row.try_get("endpoint").unwrap_or_default(),
        enabled: enabled != 0,
        config: parse_config(&row.try_get::<String, _>("config_json").unwrap_or_default()),
        created_at: parse_timestamp(&row.try_get::<String, _>("created_at").unwrap_or_default()),
    }
}

/// Registers a destination, or updates it if the code is already known.
///
/// Configuration is re-read on every boot, so this has to be idempotent on `code`:
/// restarting a hub must not accumulate duplicate destinations, and editing an endpoint
/// must take effect without losing the delivery history keyed on the destination id.
pub async fn upsert_destination(
    pool: &SqlitePool,
    destination: &NewDestination,
) -> Result<DestinationRow, PoolError> {
    let now = Utc::now().to_rfc3339();
    let config = destination.config.to_string();
    observed("upsert_outbox_destination", async {
        sqlx::query(
            "INSERT INTO outbox_destinations (id, code, kind, endpoint, enabled, config_json, created_at) \
             VALUES (?, ?, ?, ?, 1, ?, ?) \
             ON CONFLICT(code) DO UPDATE SET kind = excluded.kind, endpoint = excluded.endpoint, \
             config_json = excluded.config_json, enabled = 1",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&destination.code)
        .bind(&destination.kind)
        .bind(destination.endpoint.as_deref())
        .bind(&config)
        .bind(&now)
        .execute(pool)
        .await
    })
    .await?;

    let row = sqlx::query(&format!(
        "SELECT {DESTINATION_COLUMNS} FROM outbox_destinations WHERE code = ?"
    ))
    .bind(&destination.code)
    .fetch_one(pool)
    .await?;
    Ok(row_to_destination(&row))
}

pub async fn list_enabled_destinations(
    pool: &SqlitePool,
) -> Result<Vec<DestinationRow>, PoolError> {
    let rows = observed("list_outbox_destinations", async {
        sqlx::query(&format!(
            "SELECT {DESTINATION_COLUMNS} FROM outbox_destinations WHERE enabled = 1 ORDER BY code"
        ))
        .fetch_all(pool)
        .await
    })
    .await?;
    Ok(rows.iter().map(row_to_destination).collect())
}

/// Stops owing a destination anything new. Deliveries already owed stay owed: turning a
/// destination off is not a licence to forget submissions it has not taken yet.
pub async fn disable_destination(pool: &SqlitePool, code: &str) -> Result<(), PoolError> {
    observed("disable_outbox_destination", async {
        sqlx::query("UPDATE outbox_destinations SET enabled = 0 WHERE code = ?")
            .bind(code)
            .execute(pool)
            .await
    })
    .await?;
    Ok(())
}

/// A submission still owed to someone, with the payload needed to decide who wants it.
#[derive(Debug, Clone)]
pub struct PendingSubmission {
    pub id: Uuid,
    pub payload: String,
}

/// Submissions whose summary status is still `pending`, oldest first.
///
/// Unlike `fetch_pending_outbox` this ignores the row-level `next_retry_at`: retry timing
/// is per-destination now, so the submission itself is always a fan-out candidate until
/// every destination has finished with it.
pub async fn pending_submissions(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<PendingSubmission>, PoolError> {
    let rows = observed("pending_outbox_submissions", async {
        sqlx::query(
            "SELECT id, payload FROM outbox WHERE status = 'pending' ORDER BY created_at LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await
    })
    .await?;
    Ok(rows
        .iter()
        .map(|row| PendingSubmission {
            id: parse_uuid(&row.try_get::<String, _>("id").unwrap_or_default()),
            payload: row.try_get("payload").unwrap_or_default(),
        })
        .collect())
}

/// Records that a destination is owed a submission. `Ok(false)` means it already was.
///
/// Fan-out runs every cycle and a submission can stay pending for many cycles while a
/// destination is unreachable, so this has to be idempotent: the unique index on
/// (outbox_id, destination_id) is what turns a repeat into a no-op rather than a second
/// delivery of the same sale.
pub async fn ensure_delivery(
    pool: &SqlitePool,
    outbox_id: Uuid,
    destination_id: Uuid,
) -> Result<bool, PoolError> {
    let now = Utc::now().to_rfc3339();
    let result = observed("ensure_outbox_delivery", async {
        sqlx::query(
            "INSERT OR IGNORE INTO outbox_delivery_attempts \
                 (id, outbox_id, destination_id, status, attempts, next_attempt_at, created_at, updated_at) \
             VALUES (?, ?, ?, 'pending', 0, NULL, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(outbox_id.to_string())
        .bind(destination_id.to_string())
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await
    })
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Deliveries that should be attempted now, oldest submission first.
pub async fn fetch_due_deliveries(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<DueDelivery>, PoolError> {
    let now = Utc::now().to_rfc3339();
    let rows = observed("fetch_due_outbox_deliveries", async {
        sqlx::query(
            "SELECT a.id AS attempt_id, a.outbox_id, a.destination_id, a.attempts, a.last_error, \
                    o.payload, d.code, d.kind, d.endpoint, d.config_json \
             FROM outbox_delivery_attempts a \
             JOIN outbox o ON o.id = a.outbox_id \
             JOIN outbox_destinations d ON d.id = a.destination_id \
             WHERE a.status = 'pending' AND d.enabled = 1 \
               AND (a.next_attempt_at IS NULL OR a.next_attempt_at <= ?) \
             ORDER BY o.created_at LIMIT ?",
        )
        .bind(&now)
        .bind(limit)
        .fetch_all(pool)
        .await
    })
    .await?;

    Ok(rows
        .iter()
        .map(|row| DueDelivery {
            attempt_id: parse_uuid(&row.try_get::<String, _>("attempt_id").unwrap_or_default()),
            outbox_id: parse_uuid(&row.try_get::<String, _>("outbox_id").unwrap_or_default()),
            payload: row.try_get("payload").unwrap_or_default(),
            destination_id: parse_uuid(
                &row.try_get::<String, _>("destination_id")
                    .unwrap_or_default(),
            ),
            destination_code: row.try_get("code").unwrap_or_default(),
            destination_kind: row.try_get("kind").unwrap_or_default(),
            endpoint: row.try_get("endpoint").unwrap_or_default(),
            config: parse_config(&row.try_get::<String, _>("config_json").unwrap_or_default()),
            attempts: row.try_get("attempts").unwrap_or_default(),
            last_error: row.try_get("last_error").unwrap_or_default(),
        })
        .collect())
}

pub async fn mark_delivery_delivered(pool: &SqlitePool, attempt_id: Uuid) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    observed("mark_outbox_delivery_delivered", async {
        sqlx::query(
            "UPDATE outbox_delivery_attempts \
             SET status = 'delivered', attempts = attempts + 1, next_attempt_at = NULL, \
                 last_error = NULL, updated_at = ? \
             WHERE id = ?",
        )
        .bind(&now)
        .bind(attempt_id.to_string())
        .execute(pool)
        .await
    })
    .await?;
    Ok(())
}

pub async fn schedule_delivery_retry(
    pool: &SqlitePool,
    attempt_id: Uuid,
    next_attempt_at: DateTime<Utc>,
    error: &str,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    observed("schedule_outbox_delivery_retry", async {
        sqlx::query(
            "UPDATE outbox_delivery_attempts \
             SET attempts = attempts + 1, next_attempt_at = ?, last_error = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(next_attempt_at.to_rfc3339())
        .bind(error)
        .bind(&now)
        .bind(attempt_id.to_string())
        .execute(pool)
        .await
    })
    .await?;
    Ok(())
}

/// Gives up on one destination. The delivery is never retried automatically again, which
/// is the point: an endless retry loop hides a failure behind a queue that never drains.
pub async fn mark_delivery_dead_letter(
    pool: &SqlitePool,
    attempt_id: Uuid,
    reason: &str,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    observed("mark_outbox_delivery_dead_letter", async {
        sqlx::query(
            "UPDATE outbox_delivery_attempts \
             SET status = 'dead_letter', attempts = attempts + 1, next_attempt_at = NULL, \
                 last_error = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(reason)
        .bind(&now)
        .bind(attempt_id.to_string())
        .execute(pool)
        .await
    })
    .await?;
    Ok(())
}

/// Re-queues a dead-lettered delivery after an operator has fixed whatever broke.
pub async fn retry_dead_letter_delivery(
    pool: &SqlitePool,
    attempt_id: Uuid,
) -> Result<bool, PoolError> {
    let now = Utc::now().to_rfc3339();
    let result = observed("retry_outbox_dead_letter", async {
        sqlx::query(
            "UPDATE outbox_delivery_attempts \
             SET status = 'pending', attempts = 0, next_attempt_at = NULL, updated_at = ? \
             WHERE id = ? AND status = 'dead_letter'",
        )
        .bind(&now)
        .bind(attempt_id.to_string())
        .execute(pool)
        .await
    })
    .await?;
    if result.rows_affected() > 0 {
        // The submission is owed again, so it must leave the dead-letter state with it.
        sqlx::query("UPDATE outbox SET status = 'pending', error_message = NULL WHERE id = (SELECT outbox_id FROM outbox_delivery_attempts WHERE id = ?)")
            .bind(attempt_id.to_string())
            .execute(pool)
            .await?;
    }
    Ok(result.rows_affected() > 0)
}

const ATTEMPT_COLUMNS: &str = "a.id AS attempt_id, a.outbox_id, a.destination_id, d.code, \
     a.status, a.attempts, a.last_error, a.next_attempt_at, a.updated_at";

fn row_to_attempt(row: &sqlx::sqlite::SqliteRow) -> DeliveryAttemptRow {
    let next_attempt_at: Option<String> = row.try_get("next_attempt_at").unwrap_or_default();
    DeliveryAttemptRow {
        attempt_id: parse_uuid(&row.try_get::<String, _>("attempt_id").unwrap_or_default()),
        outbox_id: parse_uuid(&row.try_get::<String, _>("outbox_id").unwrap_or_default()),
        destination_id: parse_uuid(
            &row.try_get::<String, _>("destination_id")
                .unwrap_or_default(),
        ),
        destination_code: row.try_get("code").unwrap_or_default(),
        state: DeliveryState::from_str(&row.try_get::<String, _>("status").unwrap_or_default()),
        attempts: row.try_get("attempts").unwrap_or_default(),
        last_error: row.try_get("last_error").unwrap_or_default(),
        next_attempt_at: next_attempt_at.as_deref().map(parse_timestamp),
        updated_at: parse_timestamp(&row.try_get::<String, _>("updated_at").unwrap_or_default()),
    }
}

pub async fn delivery_attempts_for_submission(
    pool: &SqlitePool,
    outbox_id: Uuid,
) -> Result<Vec<DeliveryAttemptRow>, PoolError> {
    let rows = observed("list_outbox_delivery_attempts", async {
        sqlx::query(&format!(
            "SELECT {ATTEMPT_COLUMNS} FROM outbox_delivery_attempts a \
             JOIN outbox_destinations d ON d.id = a.destination_id \
             WHERE a.outbox_id = ? ORDER BY d.code"
        ))
        .bind(outbox_id.to_string())
        .fetch_all(pool)
        .await
    })
    .await?;
    Ok(rows.iter().map(row_to_attempt).collect())
}

/// Everything an operator has to deal with by hand.
pub async fn dead_letter_deliveries(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<DeliveryAttemptRow>, PoolError> {
    let rows = observed("list_outbox_dead_letters", async {
        sqlx::query(&format!(
            "SELECT {ATTEMPT_COLUMNS} FROM outbox_delivery_attempts a \
             JOIN outbox_destinations d ON d.id = a.destination_id \
             WHERE a.status = 'dead_letter' ORDER BY a.updated_at DESC LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(pool)
        .await
    })
    .await?;
    Ok(rows.iter().map(row_to_attempt).collect())
}

/// Queue depth per state, for the gauges an operator watches.
pub async fn count_deliveries_in_state(
    pool: &SqlitePool,
    state: DeliveryState,
) -> Result<i64, PoolError> {
    let row = observed("count_outbox_deliveries", async {
        sqlx::query("SELECT COUNT(*) AS total FROM outbox_delivery_attempts WHERE status = ?")
            .bind(state.as_str())
            .fetch_one(pool)
            .await
    })
    .await?;
    Ok(row.try_get("total").unwrap_or_default())
}

/// Rolls per-destination outcomes up onto the submission.
///
/// A submission is only finished when no destination is still owed it. If any destination
/// gave up, the submission is dead-lettered even though others took it — half-delivered
/// is a failure an operator has to see, not a success.
pub async fn settle_finished_submissions(
    pool: &SqlitePool,
    limit: i64,
) -> Result<usize, PoolError> {
    let candidates = observed("settle_outbox_submissions", async {
        sqlx::query(
            "SELECT a.outbox_id, \
                    SUM(CASE WHEN a.status = 'pending' THEN 1 ELSE 0 END) AS pending, \
                    SUM(CASE WHEN a.status = 'dead_letter' THEN 1 ELSE 0 END) AS dead \
             FROM outbox_delivery_attempts a \
             JOIN outbox o ON o.id = a.outbox_id \
             WHERE o.status = 'pending' \
             GROUP BY a.outbox_id HAVING pending = 0 LIMIT ?",
        )
        .bind(limit)
        .fetch_all(pool)
        .await
    })
    .await?;

    let mut settled = 0;
    for row in &candidates {
        let outbox_id: String = row.try_get("outbox_id").unwrap_or_default();
        let dead: i64 = row.try_get("dead").unwrap_or_default();
        if dead > 0 {
            sqlx::query("UPDATE outbox SET status = 'dead_letter', error_message = ? WHERE id = ?")
                .bind(format!("{dead} destination(s) gave up"))
                .bind(&outbox_id)
                .execute(pool)
                .await?;
        } else {
            sqlx::query("UPDATE outbox SET status = 'delivered' WHERE id = ?")
                .bind(&outbox_id)
                .execute(pool)
                .await?;
        }
        settled += 1;
    }
    Ok(settled)
}

/// The submission's summary status, which is what the rest of the system reads.
pub async fn outbox_row_status(
    pool: &SqlitePool,
    outbox_id: Uuid,
) -> Result<Option<String>, PoolError> {
    let row = sqlx::query("SELECT status FROM outbox WHERE id = ?")
        .bind(outbox_id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| row.try_get("status").unwrap_or_default()))
}
