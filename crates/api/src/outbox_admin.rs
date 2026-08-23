//! Operator endpoints for the outbox.
//!
//! Submissions fan out to several destinations, each of which can fall behind or give up
//! on its own. A dead-letter queue nobody can see is the same as dropping the data, so
//! these endpoints answer two questions: what has not been delivered, and can it be sent
//! again now that the cause is fixed.

use apex_edge_storage::outbox_destinations::{
    dead_letter_deliveries, list_enabled_destinations, retry_dead_letter_delivery,
};
use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use crate::pos::AppState;

/// The most dead letters one response will carry. An operator works through a page at a
/// time, and an unbounded list of a broken destination's backlog helps nobody.
const DEAD_LETTER_PAGE: i64 = 100;

#[derive(Debug, Serialize)]
pub struct DeadLetterEntry {
    pub attempt_id: Uuid,
    pub outbox_id: Uuid,
    pub destination_code: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub failed_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct DestinationSummary {
    pub code: String,
    pub kind: String,
    pub endpoint: Option<String>,
    /// Submissions this destination has not taken yet.
    pub pending: i64,
    /// Submissions this destination gave up on, waiting on an operator.
    pub dead_letters: i64,
}

#[derive(Debug, Serialize)]
pub struct RetryOutcome {
    pub attempt_id: Uuid,
    /// False when the delivery was not a dead letter, so nothing was requeued.
    pub requeued: bool,
}

/// Everything a destination gave up on.
pub async fn list_outbox_dead_letters(
    State(app): State<AppState>,
) -> Result<Json<Vec<DeadLetterEntry>>, axum::http::StatusCode> {
    let rows = dead_letter_deliveries(&app.pool, DEAD_LETTER_PAGE)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "could not read outbox dead letters");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(
        rows.into_iter()
            .map(|row| DeadLetterEntry {
                attempt_id: row.attempt_id,
                outbox_id: row.outbox_id,
                destination_code: row.destination_code,
                attempts: row.attempts,
                last_error: row.last_error,
                failed_at: row.updated_at,
            })
            .collect(),
    ))
}

/// Where submissions go, and how far behind each place is.
pub async fn list_outbox_destinations(
    State(app): State<AppState>,
) -> Result<Json<Vec<DestinationSummary>>, axum::http::StatusCode> {
    let destinations = list_enabled_destinations(&app.pool).await.map_err(|e| {
        tracing::error!(error = %e, "could not read outbox destinations");
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut summaries = Vec::with_capacity(destinations.len());
    for destination in destinations {
        let row = sqlx::query(
            "SELECT SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END) AS pending, \
                    SUM(CASE WHEN status = 'dead_letter' THEN 1 ELSE 0 END) AS dead \
             FROM outbox_delivery_attempts WHERE destination_id = ?",
        )
        .bind(destination.id.to_string())
        .fetch_one(&app.pool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "could not count outbox deliveries");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
        summaries.push(DestinationSummary {
            code: destination.code,
            kind: destination.kind,
            endpoint: destination.endpoint,
            pending: row.try_get("pending").unwrap_or_default(),
            dead_letters: row.try_get("dead").unwrap_or_default(),
        });
    }
    Ok(Json(summaries))
}

/// Sends a dead-lettered submission again, from the start of its backoff.
pub async fn retry_outbox_dead_letter(
    State(app): State<AppState>,
    Path(attempt_id): Path<Uuid>,
) -> Result<Json<RetryOutcome>, axum::http::StatusCode> {
    let requeued = retry_dead_letter_delivery(&app.pool, attempt_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "could not requeue outbox dead letter");
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if requeued {
        tracing::info!(attempt_id = %attempt_id, "outbox dead letter requeued by operator");
    }
    Ok(Json(RetryOutcome {
        attempt_id,
        requeued,
    }))
}
