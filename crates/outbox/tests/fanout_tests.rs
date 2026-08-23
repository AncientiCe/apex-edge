//! Multi-destination outbox dispatch.
//!
//! One sale can be owed to HQ, a Peppol access point and a KSeF gateway at once. What
//! matters is that each of them gets it exactly once, that a broken one cannot stop a
//! working one, and that nothing is ever quietly dropped.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use apex_edge_outbox::{payload_kind, run_once, DispatcherPolicy};
use apex_edge_storage::{
    dead_letter_deliveries, delivery_attempts_for_submission, insert_outbox,
    list_enabled_destinations, outbox_row_status, run_migrations, upsert_destination,
    DeliveryState, NewDestination,
};
use axum::{extract::State, routing::post, Json, Router};
use reqwest::Client;
use serde_json::{json, Value};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use uuid::Uuid;

/// A destination that records what it received.
#[derive(Clone, Default)]
struct Recorder {
    received: Arc<AtomicUsize>,
}

impl Recorder {
    fn count(&self) -> usize {
        self.received.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Copy)]
enum Behaviour {
    Accept,
    Reject,
    ServerError,
}

/// Spawns a destination server and returns its URL plus a receipt counter.
async fn destination_server(behaviour: Behaviour) -> (String, Recorder) {
    let recorder = Recorder::default();
    let handler_recorder = recorder.clone();
    let app = Router::new().route(
        "/submit",
        post(
            move |State(recorder): State<Recorder>, Json(_body): Json<Value>| async move {
                recorder.received.fetch_add(1, Ordering::SeqCst);
                match behaviour {
                    Behaviour::Accept => (
                        axum::http::StatusCode::OK,
                        Json(json!({
                            "accepted": true,
                            "submission_id": Uuid::nil(),
                            "order_id": Uuid::nil(),
                            "hq_order_ref": "HQ-1",
                            "errors": []
                        })),
                    ),
                    Behaviour::Reject => (
                        axum::http::StatusCode::OK,
                        Json(json!({
                            "accepted": false,
                            "submission_id": Uuid::nil(),
                            "order_id": Uuid::nil(),
                            "hq_order_ref": "",
                            "errors": [{"code": "DUPLICATE", "message": "duplicate sequence number"}]
                        })),
                    ),
                    Behaviour::ServerError => (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({})),
                    ),
                }
            },
        ),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let served = app.with_state(handler_recorder);
    tokio::spawn(async move {
        let _ = axum::serve(listener, served).await;
    });
    (format!("http://127.0.0.1:{port}/submit"), recorder)
}

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    pool
}

async fn register(pool: &SqlitePool, code: &str, endpoint: &str, config: Value) {
    upsert_destination(
        pool,
        &NewDestination {
            code: code.into(),
            kind: "http".into(),
            endpoint: Some(endpoint.into()),
            config,
        },
    )
    .await
    .expect("register destination");
}

async fn queue_order(pool: &SqlitePool) -> Uuid {
    let id = Uuid::new_v4();
    insert_outbox(pool, id, &json!({"order": {"order_id": id}}).to_string())
        .await
        .expect("insert outbox");
    id
}

fn policy() -> DispatcherPolicy {
    DispatcherPolicy::default()
}

#[tokio::test]
async fn every_destination_receives_the_submission() {
    let pool = pool().await;
    let (hq_url, hq) = destination_server(Behaviour::Accept).await;
    let (peppol_url, peppol) = destination_server(Behaviour::Accept).await;
    register(&pool, "hq", &hq_url, json!({})).await;
    register(&pool, "peppol", &peppol_url, json!({})).await;
    let outbox_id = queue_order(&pool).await;

    let delivered = run_once(&pool, &Client::new(), &policy())
        .await
        .expect("dispatch");

    assert_eq!(delivered, 2, "both destinations were delivered to");
    assert_eq!(hq.count(), 1);
    assert_eq!(peppol.count(), 1);
    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("delivered".to_string())
    );
}

#[tokio::test]
async fn a_destination_that_already_has_it_is_not_sent_it_again() {
    // The dispatcher runs every 30 seconds forever. Re-sending a completed delivery would
    // duplicate sales at HQ.
    let pool = pool().await;
    let (hq_url, hq) = destination_server(Behaviour::Accept).await;
    register(&pool, "hq", &hq_url, json!({})).await;
    queue_order(&pool).await;

    run_once(&pool, &Client::new(), &policy())
        .await
        .expect("dispatch");
    run_once(&pool, &Client::new(), &policy())
        .await
        .expect("dispatch");

    assert_eq!(hq.count(), 1);
}

#[tokio::test]
async fn a_broken_destination_does_not_stop_a_working_one() {
    let pool = pool().await;
    let (hq_url, hq) = destination_server(Behaviour::Accept).await;
    let (broken_url, broken) = destination_server(Behaviour::ServerError).await;
    register(&pool, "hq", &hq_url, json!({})).await;
    register(&pool, "peppol", &broken_url, json!({})).await;
    let outbox_id = queue_order(&pool).await;

    let delivered = run_once(&pool, &Client::new(), &policy())
        .await
        .expect("dispatch");

    assert_eq!(delivered, 1);
    assert_eq!(hq.count(), 1);
    assert_eq!(broken.count(), 1);
    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("pending".to_string()),
        "the submission is still owed to the broken destination"
    );

    let attempts = delivery_attempts_for_submission(&pool, outbox_id)
        .await
        .expect("attempts");
    let hq_attempt = attempts
        .iter()
        .find(|a| a.destination_code == "hq")
        .expect("hq");
    let peppol_attempt = attempts
        .iter()
        .find(|a| a.destination_code == "peppol")
        .expect("peppol");
    assert_eq!(hq_attempt.state, DeliveryState::Delivered);
    assert_eq!(peppol_attempt.state, DeliveryState::Pending);
    assert!(
        peppol_attempt.next_attempt_at.is_some(),
        "a failed delivery must be scheduled to try again"
    );
}

#[tokio::test]
async fn a_destination_that_keeps_failing_ends_up_in_the_dead_letter_queue() {
    // Retrying forever hides the failure behind a queue that never drains.
    let pool = pool().await;
    let (broken_url, _) = destination_server(Behaviour::ServerError).await;
    register(&pool, "hq", &broken_url, json!({})).await;
    let outbox_id = queue_order(&pool).await;
    let policy = DispatcherPolicy {
        max_attempts: 3,
        // No backoff, so the test does not have to wait out a real one.
        base_backoff_seconds: 0,
        ..DispatcherPolicy::default()
    };

    for _ in 0..4 {
        run_once(&pool, &Client::new(), &policy)
            .await
            .expect("dispatch");
    }

    let dlq = dead_letter_deliveries(&pool, 10).await.expect("dlq");
    assert_eq!(
        dlq.len(),
        1,
        "the delivery gave up after its attempts ran out"
    );
    assert_eq!(dlq[0].destination_code, "hq");
    assert!(dlq[0]
        .last_error
        .as_deref()
        .expect("an error the operator can act on")
        .contains("500"));
    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("dead_letter".to_string())
    );
}

#[tokio::test]
async fn a_destination_that_rejects_the_payload_also_ends_up_dead_lettered() {
    // A rejection is not a transient failure: retrying an invalid payload forever is
    // pointless, and the old dispatcher retried these indefinitely.
    let pool = pool().await;
    let (rejecting_url, rejecting) = destination_server(Behaviour::Reject).await;
    register(&pool, "hq", &rejecting_url, json!({})).await;
    let outbox_id = queue_order(&pool).await;
    let policy = DispatcherPolicy {
        max_attempts: 2,
        base_backoff_seconds: 0,
        ..DispatcherPolicy::default()
    };

    for _ in 0..3 {
        run_once(&pool, &Client::new(), &policy)
            .await
            .expect("dispatch");
    }

    let attempts = delivery_attempts_for_submission(&pool, outbox_id)
        .await
        .expect("attempts");
    assert_eq!(
        rejecting.count(),
        2,
        "a rejection is retried once, then given up on: {attempts:?}"
    );
    let dlq = dead_letter_deliveries(&pool, 10).await.expect("dlq");
    assert_eq!(dlq.len(), 1);
    assert!(dlq[0]
        .last_error
        .as_deref()
        .expect("rejection reason")
        .contains("duplicate sequence number"));
}

#[tokio::test]
async fn a_destination_only_receives_the_payload_kinds_it_asked_for() {
    // A Peppol access point wants invoices, not till counts. Sending everything to
    // everywhere is how an integration becomes a liability.
    let pool = pool().await;
    let (hq_url, hq) = destination_server(Behaviour::Accept).await;
    let (peppol_url, peppol) = destination_server(Behaviour::Accept).await;
    register(&pool, "hq", &hq_url, json!({})).await;
    register(
        &pool,
        "peppol",
        &peppol_url,
        json!({"payload_kinds": ["order"]}),
    )
    .await;

    queue_order(&pool).await;
    let shift_id = Uuid::new_v4();
    insert_outbox(
        &pool,
        shift_id,
        &json!({"shift": {"id": shift_id}}).to_string(),
    )
    .await
    .expect("insert shift");

    run_once(&pool, &Client::new(), &policy())
        .await
        .expect("dispatch");

    assert_eq!(hq.count(), 2, "HQ asked for everything");
    assert_eq!(peppol.count(), 1, "the access point asked for orders only");
    assert_eq!(
        outbox_row_status(&pool, shift_id).await.expect("status"),
        Some("delivered".to_string()),
        "a submission nobody else wanted is still finished"
    );
}

#[tokio::test]
async fn a_hub_with_no_destinations_keeps_its_submissions() {
    // Losing a sale because nothing is configured yet would be the worst possible
    // failure mode. It queues, and it waits.
    let pool = pool().await;
    let outbox_id = queue_order(&pool).await;

    let delivered = run_once(&pool, &Client::new(), &policy())
        .await
        .expect("dispatch");

    assert_eq!(delivered, 0);
    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("pending".to_string())
    );
}

#[tokio::test]
async fn an_unreachable_destination_is_retried_not_dropped() {
    let pool = pool().await;
    // Port 1 refuses connections on every platform CI runs on.
    register(&pool, "hq", "http://127.0.0.1:1/submit", json!({})).await;
    let outbox_id = queue_order(&pool).await;

    run_once(&pool, &Client::new(), &policy())
        .await
        .expect("dispatch");

    let attempts = delivery_attempts_for_submission(&pool, outbox_id)
        .await
        .expect("attempts");
    assert_eq!(attempts[0].state, DeliveryState::Pending);
    assert_eq!(attempts[0].attempts, 1);
    assert!(attempts[0].last_error.is_some());
}

#[tokio::test]
async fn the_hq_url_registers_a_destination_so_existing_deployments_keep_working() {
    // Operators upgrading to fan-out have APEX_EDGE_HQ_SUBMIT_URL set and nothing else.
    // That must keep meaning "send sales to HQ".
    let pool = pool().await;
    let (hq_url, hq) = destination_server(Behaviour::Accept).await;

    apex_edge_outbox::register_hq_destination(&pool, &hq_url)
        .await
        .expect("register hq");
    queue_order(&pool).await;
    run_once(&pool, &Client::new(), &policy())
        .await
        .expect("dispatch");

    let destinations = list_enabled_destinations(&pool).await.expect("list");
    assert_eq!(destinations.len(), 1);
    assert_eq!(destinations[0].code, "hq");
    assert_eq!(hq.count(), 1);
}

#[test]
fn payload_kinds_are_recognised_from_the_submission_itself() {
    // Destinations filter on these names, so a payload the hub cannot classify must be a
    // visible "unknown" rather than silently matching a filter.
    assert_eq!(payload_kind(&json!({"order": {}})), "order");
    assert_eq!(payload_kind(&json!({"ret": {}})), "return");
    assert_eq!(payload_kind(&json!({"shift": {}})), "shift");
    assert_eq!(
        payload_kind(&json!({"event_type": "stock.movement"})),
        "stock.movement"
    );
    assert_eq!(payload_kind(&json!({"nothing": true})), "unknown");
}
