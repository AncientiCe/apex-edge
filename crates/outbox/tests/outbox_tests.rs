use apex_edge_outbox::{run_dispatcher_loop, run_once};
use apex_edge_storage::{fetch_pending_outbox, insert_outbox, run_migrations};
use axum::{routing::post, Json, Router};
use reqwest::Client;
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::net::TcpListener;
use uuid::Uuid;

const MAX_ATTEMPTS: i32 = 10;

#[tokio::test]
async fn dispatcher_submits_and_marks_outbox_delivered() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    insert_outbox(&pool, Uuid::new_v4(), r#"{"order_id":"1"}"#)
        .await
        .expect("insert outbox");

    let app = Router::new().route(
        "/submit",
        post(|| async {
            Json(json!({
                "accepted": true,
                "submission_id": "00000000-0000-0000-0000-000000000000",
                "order_id": "00000000-0000-0000-0000-000000000000",
                "hq_order_ref": "HQ-1",
                "errors": []
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let processed = run_once(
        &pool,
        &Client::new(),
        &format!("http://127.0.0.1:{port}/submit"),
    )
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
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let id = Uuid::new_v4();
    insert_outbox(&pool, id, r#"{"order_id":"2"}"#)
        .await
        .expect("insert outbox");

    let app = Router::new().route(
        "/submit",
        post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "nope") }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let processed = run_once(
        &pool,
        &Client::new(),
        &format!("http://127.0.0.1:{port}/submit"),
    )
    .await
    .expect("dispatcher run");
    assert_eq!(processed, 0);

    let row: (String, i32) = sqlx::query_as("SELECT status, attempts FROM outbox WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .expect("outbox row");
    assert_eq!(row.0, "pending");
    assert_eq!(row.1, 1);
}

#[tokio::test]
async fn dispatcher_loop_dispatches_pending_rows_and_can_be_cancelled() {
    // Use a named shared in-memory DB so the connection persists after loop cancellation.
    let db_id = Uuid::new_v4().simple().to_string();
    let conn_str = format!("sqlite:file:{db_id}?mode=memory&cache=shared");
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&conn_str)
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    insert_outbox(&pool, Uuid::new_v4(), r#"{"order_id":"loop-test"}"#)
        .await
        .expect("insert outbox");

    let app = Router::new().route(
        "/submit",
        post(|| async {
            Json(json!({
                "accepted": true,
                "submission_id": "00000000-0000-0000-0000-000000000000",
                "order_id": "00000000-0000-0000-0000-000000000000",
                "hq_order_ref": "HQ-loop",
                "errors": []
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let url = format!("http://127.0.0.1:{port}/submit");

    // Spawn the loop and wait long enough for at least one dispatch cycle (fires immediately).
    let pool_for_loop = pool.clone();
    let handle = tokio::spawn(async move {
        run_dispatcher_loop(
            pool_for_loop,
            Client::new(),
            url,
            std::time::Duration::from_millis(10),
        )
        .await;
    });

    let pending_drained = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let pending = fetch_pending_outbox(&pool, 10)
                .await
                .expect("pending while loop running");
            if pending.is_empty() {
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
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let id = Uuid::new_v4();
    insert_outbox(&pool, id, r#"{"order_id":"dlq-test"}"#)
        .await
        .expect("insert outbox");

    // Pre-set the attempts counter to MAX_ATTEMPTS so the next failure triggers DLQ
    // (dispatcher checks `attempts >= MAX_ATTEMPTS` before deciding to DLQ or retry).
    sqlx::query("UPDATE outbox SET attempts = ? WHERE id = ?")
        .bind(MAX_ATTEMPTS)
        .bind(id.to_string())
        .execute(&pool)
        .await
        .expect("set attempts");

    // HQ always returns 500.
    let app = Router::new().route(
        "/submit",
        post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "nope") }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let processed = run_once(
        &pool,
        &Client::new(),
        &format!("http://127.0.0.1:{port}/submit"),
    )
    .await
    .expect("dispatcher run");
    assert_eq!(processed, 0, "DLQ'd row is not counted as processed");

    let row: (String, i32) = sqlx::query_as("SELECT status, attempts FROM outbox WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .expect("outbox row");
    assert_eq!(
        row.0, "dead_letter",
        "row should be moved to dead_letter status"
    );
    assert_eq!(
        row.1, MAX_ATTEMPTS,
        "attempts should equal MAX_ATTEMPTS after DLQ"
    );
}
