//! Background dispatcher: poll outbox, POST to HQ, retry with backoff + jitter, DLQ on max attempts.

use apex_edge_contracts::HqOrderSubmissionResponse;
use apex_edge_metrics::{
    OUTBOX_DISPATCHER_CYCLES_TOTAL, OUTBOX_DISPATCH_ATTEMPTS_TOTAL,
    OUTBOX_DISPATCH_DURATION_SECONDS, OUTBOX_DLQ_TOTAL, OUTCOME_ACCEPTED, OUTCOME_ERROR,
    OUTCOME_HTTP_ERROR, OUTCOME_REJECTED, OUTCOME_TIMEOUT,
};
use apex_edge_storage::outbox::OutboxRow;
use apex_edge_storage::outbox::{
    fetch_pending_outbox, mark_dead_letter, mark_delivered, schedule_retry,
};
use chrono::{Duration, Utc};
use reqwest::Client;
use sqlx::SqlitePool;
use std::time::Instant;
use thiserror::Error;
use tracing::info;

const MAX_ATTEMPTS: i32 = 10;
const BASE_BACKOFF_SECS: i64 = 5;

fn backoff_delay_secs(attempts: i32) -> i64 {
    BASE_BACKOFF_SECS * (1 << attempts.min(6))
}

#[derive(Error, Debug)]
pub enum DispatcherError {
    #[error("storage: {0}")]
    Storage(#[from] apex_edge_storage::pool::PoolError),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Run one outbox dispatch cycle.
///
/// # Examples
///
/// ```no_run
/// use apex_edge_outbox::run_once;
/// use reqwest::Client;
/// use sqlx::sqlite::SqlitePoolOptions;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let pool = SqlitePoolOptions::new()
///     .max_connections(1)
///     .connect("sqlite::memory:")
///     .await
///     .unwrap();
/// apex_edge_storage::run_migrations(&pool).await.unwrap();
///
/// let _ = run_once(&pool, &Client::new(), "http://127.0.0.1:3000/submit").await;
/// # }
/// ```
pub async fn run_once(
    pool: &SqlitePool,
    client: &Client,
    hq_submit_url: &str,
) -> Result<usize, DispatcherError> {
    let pending = fetch_pending_outbox(pool, 10).await?;
    let mut processed = 0;
    for row in pending {
        let start = Instant::now();
        let send_result = client
            .post(hq_submit_url)
            .json(&serde_json::from_str::<serde_json::Value>(&row.payload)?)
            .send()
            .await;
        let duration_secs = start.elapsed().as_secs_f64();
        metrics::histogram!(OUTBOX_DISPATCH_DURATION_SECONDS, duration_secs);

        match send_result {
            Ok(res) if res.status().is_success() => {
                let body: HqOrderSubmissionResponse = res.json().await.unwrap_or_default();
                if body.accepted {
                    metrics::counter!(OUTBOX_DISPATCH_ATTEMPTS_TOTAL, 1u64, "outcome" => OUTCOME_ACCEPTED);
                    mark_delivered(pool, row.id).await?;
                    processed += 1;
                    info!(outbox_id = %row.id, "order submitted");
                } else {
                    metrics::counter!(OUTBOX_DISPATCH_ATTEMPTS_TOTAL, 1u64, "outcome" => OUTCOME_REJECTED);
                    schedule_retry_with_backoff(pool, &row).await?;
                }
            }
            Ok(res) => {
                metrics::counter!(OUTBOX_DISPATCH_ATTEMPTS_TOTAL, 1u64, "outcome" => OUTCOME_HTTP_ERROR);
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                if row.attempts >= MAX_ATTEMPTS {
                    metrics::counter!(OUTBOX_DLQ_TOTAL, 1u64);
                    mark_dead_letter(pool, row.id, &format!("{}: {}", status, text)).await?;
                } else {
                    schedule_retry_with_backoff(pool, &row).await?;
                }
            }
            Err(e) => {
                let outcome = if e.is_timeout() {
                    OUTCOME_TIMEOUT
                } else {
                    OUTCOME_HTTP_ERROR
                };
                metrics::counter!(OUTBOX_DISPATCH_ATTEMPTS_TOTAL, 1u64, "outcome" => outcome);
                if row.attempts >= MAX_ATTEMPTS {
                    metrics::counter!(OUTBOX_DLQ_TOTAL, 1u64);
                    mark_dead_letter(pool, row.id, &e.to_string()).await?;
                } else {
                    schedule_retry_with_backoff(pool, &row).await?;
                }
            }
        }
    }
    Ok(processed)
}

async fn schedule_retry_with_backoff(
    pool: &SqlitePool,
    row: &OutboxRow,
) -> Result<(), DispatcherError> {
    let delay_secs = backoff_delay_secs(row.attempts);
    let next = Utc::now() + Duration::seconds(delay_secs);
    schedule_retry(pool, row.id, next).await?;
    Ok(())
}

/// Run the outbox dispatcher in a continuous background loop.
///
/// Fires immediately on first call (first interval tick fires at t=0), then repeats every
/// `interval`. Logs and counts errors per cycle without stopping — the loop is resilient
/// to transient HQ failures. Intended to run as a `tokio::spawn`-ed task for the lifetime
/// of the process.
pub async fn run_dispatcher_loop(
    pool: SqlitePool,
    client: Client,
    hq_submit_url: String,
    interval: std::time::Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &client, &hq_submit_url).await {
            Ok(n) => {
                if n > 0 {
                    info!(dispatched = n, "outbox dispatch cycle completed");
                }
                metrics::counter!(OUTBOX_DISPATCHER_CYCLES_TOTAL, 1u64, "outcome" => "success");
            }
            Err(e) => {
                tracing::error!(error = %e, "outbox dispatch cycle error");
                metrics::counter!(OUTBOX_DISPATCHER_CYCLES_TOTAL, 1u64, "outcome" => OUTCOME_ERROR);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::backoff_delay_secs;

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(backoff_delay_secs(0), 5);
        assert_eq!(backoff_delay_secs(1), 10);
        assert_eq!(backoff_delay_secs(6), 320);
        assert_eq!(backoff_delay_secs(10), 320);
    }
}
