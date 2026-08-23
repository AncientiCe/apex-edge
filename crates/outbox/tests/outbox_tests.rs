//! The HQ submission path, which every deployment has.
//!
//! Delivery state now lives per destination (see `fanout_tests.rs`), but the guarantee
//! these tests protect has not changed: a sale reaches HQ exactly once, a failure is
//! retried rather than lost, and a destination that never accepts it ends up somewhere an
//! operator will look.

use apex_edge_outbox::{
    register_hq_destination, run_dispatcher_loop, run_once, DispatcherPolicy, HQ_DESTINATION_CODE,
};
use apex_edge_storage::{
    dead_letter_deliveries, delivery_attempts_for_submission, fetch_pending_outbox, insert_outbox,
    run_migrations, DeliveryState,
};
use axum::{routing::post, Json, Router};
use reqwest::Client;
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use uuid::Uuid;

/// An HQ that accepts everything.
async fn accepting_hq() -> String {
    serve(Router::new().route(
        "/submit",
        post(|| async {
            Json(json!({
                "accepted": true,
                "submission_id": Uuid::nil(),
                "order_id": Uuid::nil(),
                "hq_order_ref": "HQ-1",
                "errors": []
            }))
        }),
    ))
    .await
}

/// An HQ that is having a bad day.
async fn failing_hq() -> String {
    serve(Router::new().route(
        "/submit",
        post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "nope") }),
    ))
    .await
}

async fn serve(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}/submit")
}

async fn hub_pointed_at(url: &str) -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    register_hq_destination(&pool, url).await.expect("hq");
    pool
}

#[tokio::test]
async fn dispatcher_submits_and_marks_outbox_delivered() {
    let pool = hub_pointed_at(&accepting_hq().await).await;
    insert_outbox(&pool, Uuid::new_v4(), r#"{"order":{"order_id":"1"}}"#)
        .await
        .expect("insert outbox");

    let processed = run_once(&pool, &Client::new(), &DispatcherPolicy::default())
        .await
        .expect("dispatcher run");

    assert_eq!(processed, 1);
    assert!(fetch_pending_outbox(&pool, 10)
        .await
        .expect("pending after submit")
        .is_empty());
}

#[tokio::test]
async fn dispatcher_retries_on_non_success_response() {
    let pool = hub_pointed_at(&failing_hq().await).await;
    let id = Uuid::new_v4();
    insert_outbox(&pool, id, r#"{"order":{"order_id":"2"}}"#)
        .await
        .expect("insert outbox");

    let processed = run_once(&pool, &Client::new(), &DispatcherPolicy::default())
        .await
        .expect("dispatcher run");
    assert_eq!(processed, 0);

    let status: (String,) = sqlx::query_as("SELECT status FROM outbox WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .expect("outbox row");
    assert_eq!(status.0, "pending", "the sale is still owed to HQ");

    let attempts = delivery_attempts_for_submission(&pool, id)
        .await
        .expect("attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].destination_code, HQ_DESTINATION_CODE);
    assert_eq!(attempts[0].state, DeliveryState::Pending);
    assert_eq!(attempts[0].attempts, 1);
}

#[tokio::test]
async fn dispatcher_loop_dispatches_pending_rows_and_can_be_cancelled() {
    // Named shared in-memory DB so the schema survives loop cancellation.
    let db_id = Uuid::new_v4().simple().to_string();
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&format!("sqlite:file:{db_id}?mode=memory&cache=shared"))
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    register_hq_destination(&pool, &accepting_hq().await)
        .await
        .expect("hq");
    insert_outbox(
        &pool,
        Uuid::new_v4(),
        r#"{"order":{"order_id":"loop-test"}}"#,
    )
    .await
    .expect("insert outbox");

    let pool_for_loop = pool.clone();
    let handle = tokio::spawn(async move {
        run_dispatcher_loop(
            pool_for_loop,
            Client::new(),
            DispatcherPolicy::default(),
            std::time::Duration::from_millis(10),
        )
        .await;
    });

    let pending_drained = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if fetch_pending_outbox(&pool, 10)
                .await
                .expect("pending while loop running")
                .is_empty()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    handle.abort();

    assert!(
        pending_drained,
        "outbox should be empty after dispatcher loop ran"
    );
}

#[tokio::test]
async fn dispatcher_moves_row_to_dlq_after_max_attempts() {
    let pool = hub_pointed_at(&failing_hq().await).await;
    let id = Uuid::new_v4();
    insert_outbox(&pool, id, r#"{"order":{"order_id":"dlq-test"}}"#)
        .await
        .expect("insert outbox");
    let policy = DispatcherPolicy {
        max_attempts: 3,
        base_backoff_seconds: 0,
        ..DispatcherPolicy::default()
    };

    for _ in 0..3 {
        run_once(&pool, &Client::new(), &policy)
            .await
            .expect("dispatcher run");
    }

    let status: (String, Option<String>) =
        sqlx::query_as("SELECT status, error_message FROM outbox WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(&pool)
            .await
            .expect("outbox row");
    assert_eq!(status.0, "dead_letter");
    assert!(
        status.1.is_some(),
        "the submission must say why it was given up on"
    );

    let dlq = dead_letter_deliveries(&pool, 10).await.expect("dlq");
    assert_eq!(dlq.len(), 1);
    assert_eq!(dlq[0].attempts, 3, "it used every attempt it was given");
}
