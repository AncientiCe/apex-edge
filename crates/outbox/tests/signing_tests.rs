//! Signed webhook delivery.
//!
//! A webhook receiver on the public internet cannot tell a real sale from a forged one
//! unless the hub proves it sent it. Destinations that name a signing secret get every
//! delivery signed with HMAC-SHA256 over `"{timestamp}.{body}"`, so a receiver can reject
//! both forgeries and replays of old deliveries.

use std::sync::{Arc, Mutex};

use apex_edge_outbox::{run_once, DispatcherPolicy};
use apex_edge_storage::{
    delivery_attempts_for_submission, insert_outbox, run_migrations, upsert_destination,
    DeliveryState, NewDestination,
};
use axum::{body::Bytes, extract::State, http::HeaderMap, routing::post, Router};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use uuid::Uuid;

/// Everything a receiver saw: headers and the exact bytes of the body.
#[derive(Clone, Default)]
struct Captured {
    requests: Arc<Mutex<Vec<(HeaderMap, Bytes)>>>,
}

impl Captured {
    fn all(&self) -> Vec<(HeaderMap, Bytes)> {
        self.requests.lock().expect("lock").clone()
    }
}

async fn capturing_server() -> (String, Captured) {
    let captured = Captured::default();
    let app = Router::new()
        .route(
            "/hook",
            post(
                |State(captured): State<Captured>, headers: HeaderMap, body: Bytes| async move {
                    captured
                        .requests
                        .lock()
                        .expect("lock")
                        .push((headers, body));
                    axum::http::StatusCode::OK
                },
            ),
        )
        .with_state(captured.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://127.0.0.1:{port}/hook"), captured)
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

async fn register_webhook(pool: &SqlitePool, endpoint: &str, config: Value) {
    upsert_destination(
        pool,
        &NewDestination {
            code: "analytics".into(),
            kind: "webhook".into(),
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

fn expected_signature(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("key");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[tokio::test]
async fn a_destination_with_a_signing_secret_signs_timestamp_and_exact_body() {
    std::env::set_var("APEX_EDGE_TEST_SIGNING_SECRET_OK", "s3cret");
    let pool = pool().await;
    let (url, captured) = capturing_server().await;
    register_webhook(
        &pool,
        &url,
        json!({"signing_secret_env": "APEX_EDGE_TEST_SIGNING_SECRET_OK"}),
    )
    .await;
    queue_order(&pool).await;

    let delivered = run_once(&pool, &Client::new(), &DispatcherPolicy::default())
        .await
        .expect("cycle");
    assert_eq!(delivered, 1);

    let requests = captured.all();
    assert_eq!(requests.len(), 1);
    let (headers, body) = &requests[0];
    let timestamp = headers
        .get("x-apexedge-timestamp")
        .expect("timestamp header")
        .to_str()
        .expect("ascii");
    assert!(timestamp.parse::<i64>().is_ok(), "unix seconds");
    let signature = headers
        .get("x-apexedge-signature")
        .expect("signature header")
        .to_str()
        .expect("ascii");
    assert_eq!(signature, expected_signature("s3cret", timestamp, body));
    assert_eq!(
        headers.get("content-type").map(|v| v.to_str().unwrap()),
        Some("application/json")
    );
    let parsed: Value = serde_json::from_slice(body).expect("body is json");
    assert!(parsed.get("order").is_some());
}

#[tokio::test]
async fn a_destination_without_a_signing_secret_is_sent_unsigned() {
    let pool = pool().await;
    let (url, captured) = capturing_server().await;
    register_webhook(&pool, &url, Value::Null).await;
    queue_order(&pool).await;

    run_once(&pool, &Client::new(), &DispatcherPolicy::default())
        .await
        .expect("cycle");

    let requests = captured.all();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].0.get("x-apexedge-signature").is_none());
}

#[tokio::test]
async fn a_signing_secret_that_is_not_set_fails_closed_instead_of_sending_unsigned() {
    std::env::remove_var("APEX_EDGE_TEST_SIGNING_SECRET_MISSING");
    let pool = pool().await;
    let (url, captured) = capturing_server().await;
    register_webhook(
        &pool,
        &url,
        json!({"signing_secret_env": "APEX_EDGE_TEST_SIGNING_SECRET_MISSING"}),
    )
    .await;
    let outbox_id = queue_order(&pool).await;

    let delivered = run_once(&pool, &Client::new(), &DispatcherPolicy::default())
        .await
        .expect("cycle");
    assert_eq!(delivered, 0);
    assert!(
        captured.all().is_empty(),
        "an unsigned payload must never reach a receiver expecting signatures"
    );

    let attempts = delivery_attempts_for_submission(&pool, outbox_id)
        .await
        .expect("attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].state, DeliveryState::Pending);
    assert!(attempts[0]
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("APEX_EDGE_TEST_SIGNING_SECRET_MISSING"));
}
