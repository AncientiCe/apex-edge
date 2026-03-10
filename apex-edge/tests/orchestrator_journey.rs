//! End-to-end orchestrator journey test:
//! boot app, run POS flows, and verify document retrieval journey.

use apex_edge::build_router;
use apex_edge_contracts::{ContractVersion, CreateCartPayload, PosCommand, PosRequestEnvelope};
use apex_edge_storage::{enqueue_document, mark_generated, run_migrations};
use axum::http::StatusCode;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::net::TcpListener;
use uuid::Uuid;

async fn start_app() -> (u16, sqlx::SqlitePool) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let app = build_router(pool.clone(), Uuid::nil());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = reqwest::Client::new();
    for _ in 0..30 {
        if client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .map(|r| r.status() == StatusCode::OK)
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    (port, pool)
}

#[tokio::test]
async fn orchestrator_full_journey() {
    let (port, pool) = start_app().await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // 1) Liveness/readiness journey start
    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .expect("health request");
    assert_eq!(health.status(), StatusCode::OK);

    let ready = client
        .get(format!("{base}/ready"))
        .send()
        .await
        .expect("ready request");
    assert_eq!(ready.status(), StatusCode::OK);

    // 2) Invalid contract path
    let invalid_idempotency = Uuid::new_v4();
    let invalid_req = PosRequestEnvelope {
        version: ContractVersion::new(9, 0, 0),
        idempotency_key: invalid_idempotency,
        store_id: Uuid::nil(),
        register_id: Uuid::nil(),
        payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    };
    let invalid_res = client
        .post(format!("{base}/pos/command"))
        .json(&invalid_req)
        .send()
        .await
        .expect("invalid pos request");
    assert_eq!(invalid_res.status(), StatusCode::OK);
    let invalid_json: serde_json::Value = invalid_res.json().await.expect("invalid json");
    assert_eq!(
        invalid_json.get("success"),
        Some(&serde_json::Value::Bool(false))
    );
    assert_eq!(
        invalid_json
            .get("errors")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("code"))
            .and_then(|v| v.as_str()),
        Some("UNSUPPORTED_VERSION")
    );

    // 3) Valid POS command path
    let valid_idempotency = Uuid::new_v4();
    let valid_req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: valid_idempotency,
        store_id: Uuid::nil(),
        register_id: Uuid::nil(),
        payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    };
    let valid_res = client
        .post(format!("{base}/pos/command"))
        .json(&valid_req)
        .send()
        .await
        .expect("valid pos request");
    assert_eq!(valid_res.status(), StatusCode::OK);
    let valid_json: serde_json::Value = valid_res.json().await.expect("valid json");
    assert_eq!(
        valid_json.get("success"),
        Some(&serde_json::Value::Bool(true))
    );
    assert_eq!(
        valid_json
            .get("idempotency_key")
            .and_then(|v| v.as_str())
            .expect("idempotency key in response"),
        valid_idempotency.to_string()
    );

    // 4) Document lifecycle path: seed queued->generated and verify northbound retrieval.
    let order_id = Uuid::new_v4();
    let doc_id = Uuid::new_v4();
    enqueue_document(
        &pool,
        doc_id,
        "receipt",
        Some(order_id),
        None,
        Uuid::new_v4(),
        r#"{"total_cents":1234}"#,
    )
    .await
    .expect("enqueue doc");
    mark_generated(&pool, doc_id, "text/plain", "receipt content")
        .await
        .expect("mark generated");

    let list_res = client
        .get(format!("{base}/orders/{order_id}/documents"))
        .send()
        .await
        .expect("list docs request");
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_json: serde_json::Value = list_res.json().await.expect("list docs json");
    let first = list_json
        .as_array()
        .and_then(|a| a.first())
        .expect("at least one document");
    let doc_id_str = doc_id.to_string();
    assert_eq!(
        first.get("id").and_then(|v| v.as_str()),
        Some(doc_id_str.as_str())
    );
    assert_eq!(
        first.get("status").and_then(|v| v.as_str()),
        Some("generated")
    );

    let doc_res = client
        .get(format!("{base}/documents/{doc_id}"))
        .send()
        .await
        .expect("get doc request");
    assert_eq!(doc_res.status(), StatusCode::OK);
    let doc_json: serde_json::Value = doc_res.json().await.expect("doc json");
    assert_eq!(
        doc_json.get("content").and_then(|v| v.as_str()),
        Some("receipt content")
    );
}
