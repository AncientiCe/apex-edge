//! End-to-end verification that APEX_EDGE_STANDBY=1 rejects writes with 503 and tags
//! every response with `X-ApexEdge-Role`.
//!
//! Routes test:
//! - POST /pos/command must return 503 with Retry-After and X-ApexEdge-Role: standby.
//! - GET /health must return 200 with X-ApexEdge-Role: standby.
//! - GET /audit/verify must still work (read-only + verification is exempt).

use apex_edge::build_router;
use apex_edge_api::AuthSettings;
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use reqwest::StatusCode;
use uuid::Uuid;

async fn spawn_standby() -> u16 {
    // SAFETY: tests run in the same process; setting an env var here affects the
    // role derived inside `build_router`. This test runs in isolation to avoid leaking
    // the setting across tests.
    std::env::set_var("APEX_EDGE_STANDBY", "1");
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let app = build_router(pool, Uuid::nil(), None, vec![], AuthSettings::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    std::env::remove_var("APEX_EDGE_STANDBY");
    port
}

#[tokio::test(flavor = "current_thread")]
async fn standby_rejects_writes_and_tags_role_header() {
    let port = spawn_standby().await;
    let client = reqwest::Client::new();

    // Health must still work and be tagged.
    let health = client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(
        health
            .headers()
            .get("X-ApexEdge-Role")
            .and_then(|h| h.to_str().ok()),
        Some("standby")
    );

    // Write must 503 with Retry-After.
    let write = client
        .post(format!("http://127.0.0.1:{port}/pos/command"))
        .json(&serde_json::json!({
            "version": "1.0.0",
            "idempotency_key": Uuid::new_v4(),
            "store_id": Uuid::nil(),
            "register_id": Uuid::nil(),
            "payload": { "action": "create_cart" }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(write.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        write
            .headers()
            .get("Retry-After")
            .and_then(|h| h.to_str().ok()),
        Some("30")
    );
    assert_eq!(
        write
            .headers()
            .get("X-ApexEdge-Role")
            .and_then(|h| h.to_str().ok()),
        Some("standby")
    );

    // Audit verify works (it's a read + crucial for operators during failover).
    let verify = client
        .get(format!("http://127.0.0.1:{port}/audit/verify"))
        .send()
        .await
        .unwrap();
    assert_eq!(verify.status(), StatusCode::OK);
}
