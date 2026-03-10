use apex_edge_outbox::run_once;
use apex_edge_storage::{fetch_pending_outbox, insert_outbox, run_migrations};
use axum::{routing::post, Json, Router};
use reqwest::Client;
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::net::TcpListener;
use uuid::Uuid;

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
