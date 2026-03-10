//! Smoke tests: spin up the app in-process, hit /health, /ready, and one POS path.

use apex_edge::build_router;
use apex_edge_contracts::{ContractVersion, CreateCartPayload, PosCommand, PosRequestEnvelope};
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use axum::http::StatusCode;
use tokio::net::TcpListener;
use uuid::Uuid;

/// Build app with an ephemeral SQLite DB (in-memory, shared cache for pool), run migrations, return router.
async fn app_with_ephemeral_db() -> axum::Router {
    let pool = create_sqlite_pool("sqlite:file:smoke_mem?mode=memory&cache=shared")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    build_router(pool, Uuid::nil())
}

#[tokio::test]
async fn smoke_health_returns_ok() {
    let app = app_with_ephemeral_db().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let _server = tokio::spawn(async move { axum::serve(listener, app).await });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let url = format!("http://127.0.0.1:{}/health", port);
    let client = reqwest::Client::new();
    let res = client.get(&url).send().await.expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body.get("status").and_then(|v| v.as_str()), Some("ok"));
}

#[tokio::test]
async fn smoke_ready_returns_ready() {
    let app = app_with_ephemeral_db().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let _server = tokio::spawn(async move { axum::serve(listener, app).await });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let url = format!("http://127.0.0.1:{}/ready", port);
    let client = reqwest::Client::new();
    let res = client.get(&url).send().await.expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body.get("status").and_then(|v| v.as_str()), Some("ready"));
}

#[tokio::test]
async fn smoke_pos_command_create_cart_returns_success() {
    let app = app_with_ephemeral_db().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let _server = tokio::spawn(async move { axum::serve(listener, app).await });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let envelope = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: Uuid::nil(),
        register_id: Uuid::nil(),
        payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    };
    let url = format!("http://127.0.0.1:{}/pos/command", port);
    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .json(&envelope)
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body.get("success"), Some(&serde_json::Value::Bool(true)));
}
