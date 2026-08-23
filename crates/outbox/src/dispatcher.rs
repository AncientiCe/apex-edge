//! Background dispatcher: fan every queued submission out to every configured
//! destination, with per-destination backoff and a dead-letter queue.
//!
//! A store hub owes its sales to more than one place. HQ wants them for reporting, a
//! Peppol access point or KSeF gateway wants the invoices, an analytics webhook may want
//! everything. Those endpoints fail independently, so delivery state is per destination:
//! a Peppol outage must not stop HQ receiving sales, and must not let the hub forget that
//! Peppol is still owed the same sale.
//!
//! A submission is finished only when nobody is still owed it. If a destination gives up,
//! the submission is dead-lettered even though others took it, because half-delivered is
//! a failure an operator has to see.

use apex_edge_contracts::HqOrderSubmissionResponse;
use apex_edge_metrics::{
    OUTBOX_DISPATCHER_CYCLES_TOTAL, OUTBOX_DISPATCH_ATTEMPTS_TOTAL,
    OUTBOX_DISPATCH_DURATION_SECONDS, OUTBOX_DLQ_TOTAL, OUTBOX_FANOUT_TOTAL, OUTBOX_FILTERED_TOTAL,
    OUTBOX_QUEUE_DEPTH, OUTCOME_ACCEPTED, OUTCOME_ERROR, OUTCOME_HTTP_ERROR, OUTCOME_REJECTED,
    OUTCOME_TIMEOUT,
};
use apex_edge_storage::outbox_destinations::{
    count_deliveries_in_state, ensure_delivery, fetch_due_deliveries, list_enabled_destinations,
    mark_delivery_dead_letter, mark_delivery_delivered, pending_submissions,
    schedule_delivery_retry, settle_finished_submissions, upsert_destination, DeliveryState,
    DestinationRow, DueDelivery, NewDestination,
};
use chrono::{Duration, Utc};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;
use std::time::Instant;
use thiserror::Error;
use tracing::info;

/// The destination code used for the HQ endpoint configured by
/// `APEX_EDGE_HQ_SUBMIT_URL`.
pub const HQ_DESTINATION_CODE: &str = "hq";

#[derive(Error, Debug)]
pub enum DispatcherError {
    #[error("storage: {0}")]
    Storage(#[from] apex_edge_storage::pool::PoolError),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// How hard the dispatcher tries before giving up on a destination.
#[derive(Debug, Clone)]
pub struct DispatcherPolicy {
    /// Submissions considered per cycle, and deliveries attempted per cycle.
    pub batch_size: i64,
    /// Attempts a single destination gets before its delivery is dead-lettered.
    pub max_attempts: i64,
    /// First retry delay; doubles per attempt up to six doublings.
    pub base_backoff_seconds: i64,
}

impl Default for DispatcherPolicy {
    fn default() -> Self {
        Self {
            batch_size: 10,
            max_attempts: 10,
            base_backoff_seconds: 5,
        }
    }
}

impl DispatcherPolicy {
    /// Reads the tunables an operator may need to change during an incident.
    pub fn from_env() -> Self {
        let default = Self::default();
        Self {
            batch_size: env_i64("APEX_EDGE_OUTBOX_BATCH_SIZE").unwrap_or(default.batch_size),
            max_attempts: env_i64("APEX_EDGE_OUTBOX_MAX_ATTEMPTS").unwrap_or(default.max_attempts),
            base_backoff_seconds: env_i64("APEX_EDGE_OUTBOX_BASE_BACKOFF_SECONDS")
                .unwrap_or(default.base_backoff_seconds),
        }
    }

    fn backoff_delay_seconds(&self, attempts: i64) -> i64 {
        self.base_backoff_seconds * (1 << attempts.clamp(0, 6))
    }
}

fn env_i64(key: &str) -> Option<i64> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.parse::<i64>().ok())
        .filter(|value| *value > 0)
}

/// What kind of thing a queued payload is, used to decide which destinations want it.
///
/// Derived from the payload rather than stored alongside it, so that submissions written
/// before fan-out existed classify correctly too.
pub fn payload_kind(payload: &Value) -> &'static str {
    if let Some(event_type) = payload.get("event_type").and_then(Value::as_str) {
        // Connector-style events already name themselves; the only one today is a stock
        // movement, and anything else is better reported than guessed at.
        return match event_type {
            "stock.movement" => "stock.movement",
            _ => "unknown",
        };
    }
    if payload.get("order").is_some() {
        "order"
    } else if payload.get("ret").is_some() {
        "return"
    } else if payload.get("shift").is_some() {
        "shift"
    } else {
        "unknown"
    }
}

/// Whether a destination wants this payload kind.
///
/// `payload_kinds` absent or empty means everything, which is what HQ wants and what
/// every pre-fan-out deployment gets.
fn wants(destination: &DestinationRow, kind: &str) -> bool {
    match destination
        .config
        .get("payload_kinds")
        .and_then(Value::as_array)
    {
        Some(kinds) if !kinds.is_empty() => kinds
            .iter()
            .filter_map(Value::as_str)
            .any(|wanted| wanted == kind),
        _ => true,
    }
}

/// Registers the HQ endpoint as a destination.
///
/// Deployments upgrading to fan-out have `APEX_EDGE_HQ_SUBMIT_URL` and nothing else set;
/// this keeps that meaning "send sales to HQ" without any new configuration.
pub async fn register_hq_destination(
    pool: &SqlitePool,
    submit_url: &str,
) -> Result<(), DispatcherError> {
    upsert_destination(
        pool,
        &NewDestination {
            code: HQ_DESTINATION_CODE.into(),
            kind: "http".into(),
            endpoint: Some(submit_url.to_string()),
            config: Value::Object(Default::default()),
        },
    )
    .await?;
    Ok(())
}

/// Registers destinations described by a JSON array, for everything that is not HQ.
///
/// ```json
/// [{"code":"peppol","kind":"http","endpoint":"https://ap.example/as4",
///   "config":{"payload_kinds":["order","return"]}}]
/// ```
pub async fn register_destinations_from_json(
    pool: &SqlitePool,
    raw: &str,
) -> Result<usize, DispatcherError> {
    let parsed: Vec<Value> = serde_json::from_str(raw)?;
    let mut registered = 0;
    for entry in parsed {
        let Some(code) = entry.get("code").and_then(Value::as_str) else {
            tracing::error!("outbox destination without a code ignored");
            continue;
        };
        upsert_destination(
            pool,
            &NewDestination {
                code: code.to_string(),
                kind: entry
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("http")
                    .to_string(),
                endpoint: entry
                    .get("endpoint")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                config: entry.get("config").cloned().unwrap_or(Value::Null),
            },
        )
        .await?;
        registered += 1;
    }
    Ok(registered)
}

/// Runs one dispatch cycle and returns how many deliveries succeeded.
///
/// # Examples
///
/// ```no_run
/// use apex_edge_outbox::{run_once, DispatcherPolicy};
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
/// let _ = run_once(&pool, &Client::new(), &DispatcherPolicy::default()).await;
/// # }
/// ```
pub async fn run_once(
    pool: &SqlitePool,
    client: &Client,
    policy: &DispatcherPolicy,
) -> Result<usize, DispatcherError> {
    fan_out(pool, policy).await?;

    let mut delivered = 0;
    for delivery in fetch_due_deliveries(pool, policy.batch_size).await? {
        if attempt_delivery(pool, client, policy, &delivery).await? {
            delivered += 1;
        }
    }

    settle_finished_submissions(pool, policy.batch_size).await?;
    report_queue_depth(pool).await?;
    Ok(delivered)
}

/// Owes every queued submission to every destination that wants it.
async fn fan_out(pool: &SqlitePool, policy: &DispatcherPolicy) -> Result<usize, DispatcherError> {
    let destinations = list_enabled_destinations(pool).await?;
    if destinations.is_empty() {
        return Ok(0);
    }

    let mut owed = 0;
    for submission in pending_submissions(pool, policy.batch_size).await? {
        let payload: Value = serde_json::from_str(&submission.payload).unwrap_or(Value::Null);
        let kind = payload_kind(&payload);
        for destination in &destinations {
            if !wants(destination, kind) {
                metrics::counter!(OUTBOX_FILTERED_TOTAL, "destination" => destination.code.clone(), "kind" => kind.to_string()).increment(1);
                continue;
            }
            if ensure_delivery(pool, submission.id, destination.id).await? {
                metrics::counter!(OUTBOX_FANOUT_TOTAL, "destination" => destination.code.clone())
                    .increment(1);
                owed += 1;
            }
        }
    }
    Ok(owed)
}

/// Sends one submission to one destination. `Ok(true)` means it was accepted.
async fn attempt_delivery(
    pool: &SqlitePool,
    client: &Client,
    policy: &DispatcherPolicy,
    delivery: &DueDelivery,
) -> Result<bool, DispatcherError> {
    let Some(endpoint) = delivery
        .endpoint
        .as_deref()
        .filter(|e| !e.trim().is_empty())
    else {
        // A destination with no endpoint can never be delivered to; retrying it every
        // cycle forever would just bury real failures.
        record_attempt(&delivery.destination_code, OUTCOME_ERROR);
        give_up(pool, delivery, "destination has no endpoint configured").await?;
        return Ok(false);
    };

    let body: Value = serde_json::from_str(&delivery.payload)?;
    let start = Instant::now();
    let send_result = client.post(endpoint).json(&body).send().await;
    metrics::histogram!(OUTBOX_DISPATCH_DURATION_SECONDS, "destination" => delivery.destination_code.clone())
        .record(start.elapsed().as_secs_f64());

    match send_result {
        Ok(response) if response.status().is_success() => {
            // Only HQ speaks the submission-response contract. Anything else that
            // answered 2xx has taken it, and demanding a body it does not send would
            // make every non-HQ destination undeliverable.
            let rejection = rejection_reason(response, &delivery.destination_kind).await;
            match rejection {
                None => {
                    record_attempt(&delivery.destination_code, OUTCOME_ACCEPTED);
                    mark_delivery_delivered(pool, delivery.attempt_id).await?;
                    info!(
                        outbox_id = %delivery.outbox_id,
                        destination = %delivery.destination_code,
                        "submission delivered"
                    );
                    Ok(true)
                }
                Some(reason) => {
                    // A rejection is a verdict on the payload, not a transient outage, so
                    // it is retried a few times and then handed to an operator rather
                    // than retried forever.
                    record_attempt(&delivery.destination_code, OUTCOME_REJECTED);
                    retry_or_give_up(pool, policy, delivery, &reason).await?;
                    Ok(false)
                }
            }
        }
        Ok(response) => {
            record_attempt(&delivery.destination_code, OUTCOME_HTTP_ERROR);
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            retry_or_give_up(pool, policy, delivery, &format!("{status}: {text}")).await?;
            Ok(false)
        }
        Err(e) => {
            let outcome = if e.is_timeout() {
                OUTCOME_TIMEOUT
            } else {
                OUTCOME_HTTP_ERROR
            };
            record_attempt(&delivery.destination_code, outcome);
            retry_or_give_up(pool, policy, delivery, &e.to_string()).await?;
            Ok(false)
        }
    }
}

/// `None` when the destination accepted the submission.
async fn rejection_reason(response: reqwest::Response, destination_kind: &str) -> Option<String> {
    if destination_kind == "peppol" || destination_kind == "webhook" {
        return None;
    }
    let body: HqOrderSubmissionResponse = match response.json().await {
        Ok(body) => body,
        // A 2xx with a body we cannot read is still a 2xx: treat it as accepted rather
        // than re-sending a submission the destination may already have stored.
        Err(_) => return None,
    };
    if body.accepted {
        None
    } else if body.errors.is_empty() {
        Some("rejected without a reason".into())
    } else {
        Some(
            body.errors
                .iter()
                .map(|error| format!("{}: {}", error.code, error.message))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }
}

async fn retry_or_give_up(
    pool: &SqlitePool,
    policy: &DispatcherPolicy,
    delivery: &DueDelivery,
    error: &str,
) -> Result<(), DispatcherError> {
    // `attempts` counts what has already been tried, and this attempt has just happened.
    if delivery.attempts + 1 >= policy.max_attempts {
        give_up(pool, delivery, error).await
    } else {
        let next = Utc::now() + Duration::seconds(policy.backoff_delay_seconds(delivery.attempts));
        schedule_delivery_retry(pool, delivery.attempt_id, next, error).await?;
        Ok(())
    }
}

async fn give_up(
    pool: &SqlitePool,
    delivery: &DueDelivery,
    error: &str,
) -> Result<(), DispatcherError> {
    metrics::counter!(OUTBOX_DLQ_TOTAL, "destination" => delivery.destination_code.clone())
        .increment(1);
    tracing::error!(
        outbox_id = %delivery.outbox_id,
        destination = %delivery.destination_code,
        error,
        "giving up on delivery; needs an operator"
    );
    mark_delivery_dead_letter(pool, delivery.attempt_id, error).await?;
    Ok(())
}

fn record_attempt(destination: &str, outcome: &'static str) {
    metrics::counter!(OUTBOX_DISPATCH_ATTEMPTS_TOTAL, "destination" => destination.to_string(), "outcome" => outcome).increment(1);
}

/// Publishes queue depth, which is the number that tells an operator a destination has
/// quietly stopped accepting long before anyone downstream notices missing data.
async fn report_queue_depth(pool: &SqlitePool) -> Result<(), DispatcherError> {
    for state in [DeliveryState::Pending, DeliveryState::DeadLetter] {
        let depth = count_deliveries_in_state(pool, state).await?;
        metrics::gauge!(OUTBOX_QUEUE_DEPTH, "state" => state.as_str()).set(depth as f64);
    }
    Ok(())
}

/// Runs the dispatcher until the process ends.
///
/// Fires immediately, then every `interval`. A failed cycle is logged and counted, never
/// fatal: the queue is durable, so the next cycle picks up where this one stopped.
pub async fn run_dispatcher_loop(
    pool: SqlitePool,
    client: Client,
    policy: DispatcherPolicy,
    interval: std::time::Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        match run_once(&pool, &client, &policy).await {
            Ok(n) => {
                if n > 0 {
                    info!(delivered = n, "outbox dispatch cycle completed");
                }
                metrics::counter!(OUTBOX_DISPATCHER_CYCLES_TOTAL, "outcome" => "success")
                    .increment(1);
            }
            Err(e) => {
                tracing::error!(error = %e, "outbox dispatch cycle error");
                metrics::counter!(OUTBOX_DISPATCHER_CYCLES_TOTAL, "outcome" => OUTCOME_ERROR)
                    .increment(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(config: Value) -> DestinationRow {
        DestinationRow {
            id: uuid::Uuid::nil(),
            code: "peppol".into(),
            kind: "http".into(),
            endpoint: Some("http://ap".into()),
            enabled: true,
            config,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let policy = DispatcherPolicy::default();
        assert_eq!(policy.backoff_delay_seconds(0), 5);
        assert_eq!(policy.backoff_delay_seconds(1), 10);
        assert_eq!(policy.backoff_delay_seconds(6), 320);
        assert_eq!(policy.backoff_delay_seconds(10), 320);
    }

    #[test]
    fn a_destination_with_no_filter_wants_everything() {
        let hq = destination(Value::Null);
        assert!(wants(&hq, "order"));
        assert!(wants(&hq, "shift"));
        assert!(wants(&hq, "unknown"));
    }

    #[test]
    fn an_empty_filter_is_treated_as_no_filter_rather_than_nothing() {
        // A misconfiguration that silently stopped all delivery would be far worse than
        // one that delivers too much.
        let wide_open = destination(serde_json::json!({"payload_kinds": []}));
        assert!(wants(&wide_open, "order"));
    }

    #[test]
    fn a_filtered_destination_only_wants_what_it_listed() {
        let invoices = destination(serde_json::json!({"payload_kinds": ["order", "return"]}));
        assert!(wants(&invoices, "order"));
        assert!(wants(&invoices, "return"));
        assert!(!wants(&invoices, "shift"));
        assert!(!wants(&invoices, "stock.movement"));
    }
}
