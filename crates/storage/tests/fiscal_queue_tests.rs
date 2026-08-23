//! Sign-later queue: a sale that could not be signed at the till is retried later.

use apex_edge_storage::*;
use chrono::{Duration, Utc};
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

async fn test_pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    pool
}

fn payload(transaction_id: Uuid) -> String {
    format!(r#"{{"transaction_id":"{transaction_id}"}}"#)
}

#[tokio::test]
async fn enqueueing_the_same_subject_twice_does_not_create_a_second_job() {
    let pool = test_pool().await;
    let subject_id = Uuid::new_v4();
    let entry = NewFiscalQueueEntry {
        subject_kind: "order".into(),
        subject_id,
        provider: "de_tse".into(),
        payload_json: payload(subject_id),
    };

    let first = enqueue_fiscal_signing(&pool, entry.clone())
        .await
        .expect("first enqueue");
    let second = enqueue_fiscal_signing(&pool, entry)
        .await
        .expect("retry enqueue");

    assert_eq!(first.id, second.id);
    assert_eq!(
        count_fiscal_queue(&pool, FiscalQueueStatus::Pending)
            .await
            .expect("count"),
        1
    );
}

#[tokio::test]
async fn a_due_pending_row_is_returned_and_a_future_row_is_not() {
    let pool = test_pool().await;
    let due_id = Uuid::new_v4();
    enqueue_fiscal_signing(
        &pool,
        NewFiscalQueueEntry {
            subject_kind: "order".into(),
            subject_id: due_id,
            provider: "de_tse".into(),
            payload_json: payload(due_id),
        },
    )
    .await
    .expect("enqueue due");

    let future_id = Uuid::new_v4();
    let future = enqueue_fiscal_signing(
        &pool,
        NewFiscalQueueEntry {
            subject_kind: "return".into(),
            subject_id: future_id,
            provider: "de_tse".into(),
            payload_json: payload(future_id),
        },
    )
    .await
    .expect("enqueue future");
    schedule_fiscal_retry(
        &pool,
        future.id,
        Duration::hours(1).num_seconds(),
        "waiting",
    )
    .await
    .expect("push into the future");

    let due = fetch_due_fiscal_signings(&pool, 10)
        .await
        .expect("fetch due");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].subject_id, due_id);
}

#[tokio::test]
async fn a_permanent_rejection_moves_the_row_to_the_dead_letter_queue() {
    let pool = test_pool().await;
    let subject_id = Uuid::new_v4();
    let row = enqueue_fiscal_signing(
        &pool,
        NewFiscalQueueEntry {
            subject_kind: "order".into(),
            subject_id,
            provider: "de_tse".into(),
            payload_json: payload(subject_id),
        },
    )
    .await
    .expect("enqueue");

    mark_fiscal_dead_letter(&pool, row.id, "rate not permitted")
        .await
        .expect("dead letter");

    let loaded = fetch_fiscal_queue_for_subject(&pool, "order", subject_id)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(loaded.status, FiscalQueueStatus::DeadLetter);
    assert_eq!(loaded.last_error.as_deref(), Some("rate not permitted"));
    assert!(
        fetch_due_fiscal_signings(&pool, 10)
            .await
            .expect("due")
            .is_empty(),
        "dead letters are not retried automatically"
    );
}

#[tokio::test]
async fn applying_a_receipt_clears_the_pending_flag_on_the_order() {
    let pool = test_pool().await;
    let order_id = Uuid::new_v4();
    let store_id = Uuid::new_v4();
    let register_id = Uuid::new_v4();
    insert_order_ledger_entry(
        &pool,
        &NewOrderLedgerEntry {
            order_id,
            cart_id: Uuid::new_v4(),
            store_id,
            register_id,
            shift_id: None,
            subtotal_cents: 1000,
            discount_cents: 0,
            tax_cents: 0,
            total_cents: 1000,
            submission_id: None,
            lines: vec![],
            payments: vec![],
            fiscal_provider: Some("de_tse".into()),
            fiscal_id: None,
            fiscal_signature: None,
            fiscal_qr_payload: None,
            fiscal_signed_at: None,
            fiscal_pending: true,
        },
    )
    .await
    .expect("insert pending order");

    apply_order_fiscal_receipt(
        &pool,
        order_id,
        &FiscalReceiptUpdate {
            provider: "de_tse".into(),
            fiscal_id: Some("tse_1".into()),
            signature: Some("sig_1".into()),
            qr_payload: Some("V0;tse_1".into()),
            signed_at: Some(Utc::now()),
            pending: false,
        },
    )
    .await
    .expect("apply receipt");

    let order = fetch_order_ledger_entry(&pool, order_id)
        .await
        .expect("fetch")
        .expect("order");
    assert_eq!(order.fiscal_id.as_deref(), Some("tse_1"));
    assert_eq!(order.fiscal_signature.as_deref(), Some("sig_1"));
    assert_eq!(order.fiscal_qr_payload.as_deref(), Some("V0;tse_1"));
    assert!(!order.fiscal_pending);
    assert!(order.fiscal_signed_at.is_some());
}
