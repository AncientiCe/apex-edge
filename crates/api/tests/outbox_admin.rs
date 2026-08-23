//! Operator visibility into the outbox.
//!
//! A dead-letter queue nobody can see is the same as dropping the data. These endpoints
//! are how an operator finds submissions a destination never took, and sends them again
//! once whatever broke is fixed.

use apex_edge_api::{
    list_outbox_dead_letters, list_outbox_destinations, retry_outbox_dead_letter, AppState,
};
use apex_edge_storage::{
    delivery_attempts_for_submission, ensure_delivery, insert_outbox, mark_delivery_dead_letter,
    run_migrations, upsert_destination, DeliveryState, NewDestination,
};
use axum::extract::{Path, State};
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

async fn state() -> AppState {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    AppState::new(pool, Uuid::nil())
}

/// Queues a submission and gives up on delivering it to `code`.
async fn dead_lettered(app: &AppState, code: &str) -> (Uuid, Uuid) {
    let destination = upsert_destination(
        &app.pool,
        &NewDestination {
            code: code.into(),
            kind: "http".into(),
            endpoint: Some("http://unreachable".into()),
            config: serde_json::json!({}),
        },
    )
    .await
    .expect("destination");
    let outbox_id = Uuid::new_v4();
    insert_outbox(&app.pool, outbox_id, r#"{"order":{}}"#)
        .await
        .expect("insert");
    ensure_delivery(&app.pool, outbox_id, destination.id)
        .await
        .expect("ensure");
    let attempt = delivery_attempts_for_submission(&app.pool, outbox_id)
        .await
        .expect("attempts")
        .remove(0);
    mark_delivery_dead_letter(&app.pool, attempt.attempt_id, "connection refused")
        .await
        .expect("dlq");
    (outbox_id, attempt.attempt_id)
}

#[tokio::test]
async fn the_dead_letter_queue_says_what_failed_and_why() {
    let app = state().await;
    let (outbox_id, attempt_id) = dead_lettered(&app, "peppol").await;

    let response = list_outbox_dead_letters(State(app.clone()))
        .await
        .expect("dead letters");

    assert_eq!(response.0.len(), 1);
    let entry = &response.0[0];
    assert_eq!(entry.attempt_id, attempt_id);
    assert_eq!(entry.outbox_id, outbox_id);
    assert_eq!(entry.destination_code, "peppol");
    assert_eq!(entry.last_error.as_deref(), Some("connection refused"));
}

#[tokio::test]
async fn retrying_a_dead_letter_queues_it_again() {
    let app = state().await;
    let (outbox_id, attempt_id) = dead_lettered(&app, "hq").await;

    let response = retry_outbox_dead_letter(State(app.clone()), Path(attempt_id))
        .await
        .expect("retry");

    assert!(response.0.requeued);
    let attempts = delivery_attempts_for_submission(&app.pool, outbox_id)
        .await
        .expect("attempts");
    assert_eq!(attempts[0].state, DeliveryState::Pending);
    assert!(
        list_outbox_dead_letters(State(app.clone()))
            .await
            .expect("dead letters")
            .0
            .is_empty(),
        "a requeued delivery is no longer a dead letter"
    );
}

#[tokio::test]
async fn retrying_something_that_is_not_a_dead_letter_is_reported_not_pretended() {
    let app = state().await;

    let response = retry_outbox_dead_letter(State(app.clone()), Path(Uuid::new_v4()))
        .await
        .expect("retry");

    assert!(
        !response.0.requeued,
        "an operator must not be told a retry happened when it did not"
    );
}

#[tokio::test]
async fn destinations_are_listable_so_an_operator_can_see_where_sales_go() {
    let app = state().await;
    upsert_destination(
        &app.pool,
        &NewDestination {
            code: "hq".into(),
            kind: "http".into(),
            endpoint: Some("http://hq/submit".into()),
            config: serde_json::json!({"payload_kinds": ["order"]}),
        },
    )
    .await
    .expect("destination");

    let response = list_outbox_destinations(State(app.clone()))
        .await
        .expect("destinations");

    assert_eq!(response.0.len(), 1);
    assert_eq!(response.0[0].code, "hq");
    assert_eq!(response.0[0].pending, 0);
    assert_eq!(response.0[0].dead_letters, 0);
}

#[tokio::test]
async fn a_destination_reports_how_much_it_is_behind() {
    // Queue depth per destination is the number that tells an operator which integration
    // has quietly stopped working.
    let app = state().await;
    let (_, _) = dead_lettered(&app, "peppol").await;
    let destination = upsert_destination(
        &app.pool,
        &NewDestination {
            code: "peppol".into(),
            kind: "http".into(),
            endpoint: Some("http://unreachable".into()),
            config: serde_json::json!({}),
        },
    )
    .await
    .expect("destination");
    let waiting = Uuid::new_v4();
    insert_outbox(&app.pool, waiting, r#"{"order":{}}"#)
        .await
        .expect("insert");
    ensure_delivery(&app.pool, waiting, destination.id)
        .await
        .expect("ensure");

    let response = list_outbox_destinations(State(app.clone()))
        .await
        .expect("destinations");

    let peppol = response
        .0
        .iter()
        .find(|d| d.code == "peppol")
        .expect("peppol");
    assert_eq!(peppol.pending, 1);
    assert_eq!(peppol.dead_letters, 1);
}
