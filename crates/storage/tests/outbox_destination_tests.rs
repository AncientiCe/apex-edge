//! Per-destination outbox delivery state.
//!
//! One submission now owes a delivery to every enabled destination, and each of those
//! deliveries succeeds, retries and dies independently. The properties worth pinning are
//! that fan-out cannot duplicate a delivery, that a destination which is down cannot hold
//! up one that is up, and that a submission is only finished when nobody is still owed it.

use apex_edge_storage::{
    count_deliveries_in_state, dead_letter_deliveries, delivery_attempts_for_submission,
    disable_destination, ensure_delivery, fetch_due_deliveries, insert_outbox,
    list_enabled_destinations, mark_delivery_dead_letter, mark_delivery_delivered,
    outbox_row_status, pending_submissions, retry_dead_letter_delivery, run_migrations,
    schedule_delivery_retry, settle_finished_submissions, upsert_destination, DeliveryState,
    NewDestination,
};
use chrono::{Duration, Utc};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use uuid::Uuid;

async fn pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    pool
}

fn destination(code: &str, endpoint: &str) -> NewDestination {
    NewDestination {
        code: code.into(),
        kind: "http".into(),
        endpoint: Some(endpoint.into()),
        config: serde_json::json!({}),
    }
}

async fn submission(pool: &SqlitePool, payload: &str) -> Uuid {
    let id = Uuid::new_v4();
    insert_outbox(pool, id, payload).await.expect("insert");
    id
}

/// What the dispatcher does at the start of every cycle: owe every pending submission to
/// every enabled destination.
async fn fan_out(pool: &SqlitePool, limit: i64) -> usize {
    let destinations = list_enabled_destinations(pool).await.expect("destinations");
    let mut owed = 0;
    for submission in pending_submissions(pool, limit).await.expect("pending") {
        for destination in &destinations {
            if ensure_delivery(pool, submission.id, destination.id)
                .await
                .expect("ensure")
            {
                owed += 1;
            }
        }
    }
    owed
}

#[tokio::test]
async fn a_destination_is_registered_once_no_matter_how_often_it_is_configured() {
    // Destinations come from configuration that is re-read on every boot. Restarting the
    // hub must not accumulate duplicate copies of the same destination.
    let pool = pool().await;

    let first = upsert_destination(&pool, &destination("hq", "http://hq/one"))
        .await
        .expect("upsert");
    let second = upsert_destination(&pool, &destination("hq", "http://hq/two"))
        .await
        .expect("upsert");

    assert_eq!(first.id, second.id, "the same code is the same destination");
    assert_eq!(
        second.endpoint.as_deref(),
        Some("http://hq/two"),
        "re-registering must adopt the new endpoint"
    );
    assert_eq!(
        list_enabled_destinations(&pool).await.expect("list").len(),
        1
    );
}

#[tokio::test]
async fn a_disabled_destination_is_not_owed_deliveries() {
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    upsert_destination(&pool, &destination("peppol", "http://ap"))
        .await
        .expect("upsert");
    disable_destination(&pool, "peppol").await.expect("disable");

    let outbox_id = submission(&pool, r#"{"order":{}}"#).await;
    fan_out(&pool, 10).await;

    let attempts = delivery_attempts_for_submission(&pool, outbox_id)
        .await
        .expect("attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].destination_code, "hq");
}

#[tokio::test]
async fn fanning_out_twice_does_not_owe_the_same_destination_two_deliveries() {
    // The dispatcher fans out on every cycle, and a submission can sit pending for many
    // cycles while a destination is unreachable. Duplicating here would double-submit.
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    let outbox_id = submission(&pool, r#"{"order":{}}"#).await;

    let first = fan_out(&pool, 10).await;
    let second = fan_out(&pool, 10).await;

    assert_eq!(first, 1, "the first cycle owes one delivery");
    assert_eq!(second, 0, "the second cycle owes nothing new");
    assert_eq!(
        delivery_attempts_for_submission(&pool, outbox_id)
            .await
            .expect("attempts")
            .len(),
        1
    );
}

#[tokio::test]
async fn a_destination_added_later_still_receives_older_submissions() {
    // Turning on a Peppol access point should not silently skip what is already queued.
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    let outbox_id = submission(&pool, r#"{"order":{}}"#).await;
    fan_out(&pool, 10).await;

    upsert_destination(&pool, &destination("peppol", "http://ap"))
        .await
        .expect("upsert");
    fan_out(&pool, 10).await;

    let mut codes: Vec<String> = delivery_attempts_for_submission(&pool, outbox_id)
        .await
        .expect("attempts")
        .into_iter()
        .map(|attempt| attempt.destination_code)
        .collect();
    codes.sort();
    assert_eq!(codes, vec!["hq".to_string(), "peppol".to_string()]);
}

#[tokio::test]
async fn a_retry_is_not_due_until_its_backoff_has_passed() {
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    submission(&pool, r#"{"order":{}}"#).await;
    fan_out(&pool, 10).await;
    let due = fetch_due_deliveries(&pool, 10).await.expect("due");
    assert_eq!(due.len(), 1, "a fresh delivery is due immediately");

    schedule_delivery_retry(
        &pool,
        due[0].attempt_id,
        Utc::now() + Duration::seconds(60),
        "hq said 503",
    )
    .await
    .expect("retry");

    assert!(
        fetch_due_deliveries(&pool, 10)
            .await
            .expect("due")
            .is_empty(),
        "a backed-off delivery must not be picked up early"
    );

    schedule_delivery_retry(
        &pool,
        due[0].attempt_id,
        Utc::now() - Duration::seconds(1),
        "hq said 503",
    )
    .await
    .expect("retry");
    let due_again = fetch_due_deliveries(&pool, 10).await.expect("due");
    assert_eq!(due_again.len(), 1);
    assert_eq!(
        due_again[0].attempts, 2,
        "each retry counts against the destination, not the submission"
    );
    assert_eq!(due_again[0].last_error.as_deref(), Some("hq said 503"));
}

#[tokio::test]
async fn one_destination_being_down_does_not_hold_up_another() {
    // The whole point of fan-out: a dead Peppol access point must not stop HQ receiving
    // sales, and must not make the hub forget that Peppol is still owed the same sale.
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    upsert_destination(&pool, &destination("peppol", "http://ap"))
        .await
        .expect("upsert");
    let outbox_id = submission(&pool, r#"{"order":{}}"#).await;
    fan_out(&pool, 10).await;

    let due = fetch_due_deliveries(&pool, 10).await.expect("due");
    let hq = due
        .iter()
        .find(|d| d.destination_code == "hq")
        .expect("hq delivery");
    let peppol = due
        .iter()
        .find(|d| d.destination_code == "peppol")
        .expect("peppol delivery");
    mark_delivery_delivered(&pool, hq.attempt_id)
        .await
        .expect("delivered");
    schedule_delivery_retry(
        &pool,
        peppol.attempt_id,
        Utc::now() - Duration::seconds(1),
        "connection refused",
    )
    .await
    .expect("retry");

    settle_finished_submissions(&pool, 10)
        .await
        .expect("settle");
    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("pending".to_string()),
        "a submission someone is still owed is not finished"
    );
    let still_due = fetch_due_deliveries(&pool, 10).await.expect("due");
    assert_eq!(still_due.len(), 1);
    assert_eq!(still_due[0].destination_code, "peppol");
}

#[tokio::test]
async fn a_submission_every_destination_has_taken_is_delivered() {
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    upsert_destination(&pool, &destination("peppol", "http://ap"))
        .await
        .expect("upsert");
    let outbox_id = submission(&pool, r#"{"order":{}}"#).await;
    fan_out(&pool, 10).await;

    for delivery in fetch_due_deliveries(&pool, 10).await.expect("due") {
        mark_delivery_delivered(&pool, delivery.attempt_id)
            .await
            .expect("delivered");
    }
    settle_finished_submissions(&pool, 10)
        .await
        .expect("settle");

    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("delivered".to_string())
    );
}

#[tokio::test]
async fn a_submission_no_destination_will_ever_take_is_dead_lettered() {
    // Leaving it pending forever would hide the failure behind a queue that never drains.
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    let outbox_id = submission(&pool, r#"{"order":{}}"#).await;
    fan_out(&pool, 10).await;
    let due = fetch_due_deliveries(&pool, 10).await.expect("due");

    mark_delivery_dead_letter(&pool, due[0].attempt_id, "rejected 11 times")
        .await
        .expect("dlq");
    settle_finished_submissions(&pool, 10)
        .await
        .expect("settle");

    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("dead_letter".to_string())
    );
    let dlq = dead_letter_deliveries(&pool, 10).await.expect("dlq list");
    assert_eq!(dlq.len(), 1);
    assert_eq!(dlq[0].destination_code, "hq");
    assert_eq!(dlq[0].last_error.as_deref(), Some("rejected 11 times"));
    assert!(
        fetch_due_deliveries(&pool, 10)
            .await
            .expect("due")
            .is_empty(),
        "a dead-lettered delivery must never be retried automatically"
    );
}

#[tokio::test]
async fn a_partly_dead_lettered_submission_reports_the_failure_not_success() {
    // HQ took it, Peppol never will. Calling that "delivered" would lose the compliance
    // failure; the operator has to see it.
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    upsert_destination(&pool, &destination("peppol", "http://ap"))
        .await
        .expect("upsert");
    let outbox_id = submission(&pool, r#"{"order":{}}"#).await;
    fan_out(&pool, 10).await;

    for delivery in fetch_due_deliveries(&pool, 10).await.expect("due") {
        if delivery.destination_code == "hq" {
            mark_delivery_delivered(&pool, delivery.attempt_id)
                .await
                .expect("delivered");
        } else {
            mark_delivery_dead_letter(&pool, delivery.attempt_id, "no route")
                .await
                .expect("dlq");
        }
    }
    settle_finished_submissions(&pool, 10)
        .await
        .expect("settle");

    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("dead_letter".to_string())
    );
}

#[tokio::test]
async fn an_operator_can_requeue_a_dead_letter_once_the_cause_is_fixed() {
    // A dead letter is a submission a destination never took. Once the endpoint or the
    // credentials are fixed, the operator must be able to send it without re-running the
    // sale, and the submission has to stop claiming it failed.
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    let outbox_id = submission(&pool, r#"{"order":{}}"#).await;
    fan_out(&pool, 10).await;
    let due = fetch_due_deliveries(&pool, 10).await.expect("due");
    mark_delivery_dead_letter(&pool, due[0].attempt_id, "wrong url")
        .await
        .expect("dlq");
    settle_finished_submissions(&pool, 10)
        .await
        .expect("settle");

    let requeued = retry_dead_letter_delivery(&pool, due[0].attempt_id)
        .await
        .expect("requeue");

    assert!(requeued);
    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("pending".to_string()),
        "the submission is owed again"
    );
    let due_again = fetch_due_deliveries(&pool, 10).await.expect("due");
    assert_eq!(due_again.len(), 1);
    assert_eq!(
        due_again[0].attempts, 0,
        "a requeued delivery starts its backoff over"
    );
    assert!(
        !retry_dead_letter_delivery(&pool, due[0].attempt_id)
            .await
            .expect("requeue"),
        "requeueing something already queued must report that it did nothing"
    );
}

#[tokio::test]
async fn pending_deliveries_are_countable_for_a_queue_depth_gauge() {
    let pool = pool().await;
    upsert_destination(&pool, &destination("hq", "http://hq"))
        .await
        .expect("upsert");
    submission(&pool, r#"{"order":{}}"#).await;
    submission(&pool, r#"{"order":{}}"#).await;
    fan_out(&pool, 10).await;

    assert_eq!(
        count_deliveries_in_state(&pool, DeliveryState::Pending)
            .await
            .expect("count"),
        2
    );

    let due = fetch_due_deliveries(&pool, 10).await.expect("due");
    mark_delivery_delivered(&pool, due[0].attempt_id)
        .await
        .expect("delivered");

    assert_eq!(
        count_deliveries_in_state(&pool, DeliveryState::Pending)
            .await
            .expect("count"),
        1
    );
    assert_eq!(
        count_deliveries_in_state(&pool, DeliveryState::Delivered)
            .await
            .expect("count"),
        1
    );
}

#[tokio::test]
async fn a_submission_with_no_destinations_at_all_stays_queued() {
    // A hub with nothing configured is the default. Sales must queue up and wait rather
    // than be marked delivered to nobody.
    let pool = pool().await;
    let outbox_id = submission(&pool, r#"{"order":{}}"#).await;

    assert_eq!(fan_out(&pool, 10).await, 0);
    settle_finished_submissions(&pool, 10)
        .await
        .expect("settle");

    assert_eq!(
        outbox_row_status(&pool, outbox_id).await.expect("status"),
        Some("pending".to_string())
    );
}
