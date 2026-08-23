//! Metrics that are declared, documented and graphed but never exported are worse than
//! no metrics: they make the system look observable while telling you nothing.
//!
//! This test drives the real router through the real Prometheus recorder and asserts
//! that `GET /metrics` actually contains the families the Grafana dashboards query. It
//! is the guard against the whole observability story silently going dead again — which
//! is exactly what a mismatched `metrics` crate version does.

use apex_edge::{build_router, HubConfig};
use apex_edge_contracts::pos::{CreateCartPayload, PosCommand, PosRequestEnvelope};
use apex_edge_contracts::ContractVersion;
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use axum::http::StatusCode;
use tokio::net::TcpListener;

#[tokio::test]
async fn the_metrics_endpoint_exports_what_the_dashboards_query() {
    let handle = apex_edge_metrics::install_recorder().expect("install recorder");

    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let app = build_router(
        pool,
        HubConfig {
            metrics_handle: Some(handle),
            ..HubConfig::default()
        },
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let health = client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
        .expect("health request");
    assert_eq!(health.status(), StatusCode::OK);

    // A POS command exercises the command counter behind the transaction-journey
    // dashboard.
    let command = client
        .post(format!("http://127.0.0.1:{port}/pos/command"))
        .json(&PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: uuid::Uuid::new_v4(),
            store_id: uuid::Uuid::nil(),
            register_id: uuid::Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        })
        .send()
        .await
        .expect("pos command");
    assert_eq!(command.status(), StatusCode::OK);

    let metrics = client
        .get(format!("http://127.0.0.1:{port}/metrics"))
        .send()
        .await
        .expect("metrics request")
        .text()
        .await
        .expect("metrics body");

    assert!(
        !metrics.trim().is_empty(),
        "the metrics endpoint returned nothing, so no recorder is wired up"
    );
    for family in [
        "apex_edge_http_requests_total",
        "apex_edge_http_request_duration_seconds",
        "apex_edge_pos_commands_total",
    ] {
        assert!(
            metrics.contains(family),
            "{family} is declared and graphed but was not exported:\n{metrics}"
        );
    }
    assert!(
        metrics.contains("route=\"pos_command\""),
        "route labels must be truthful, got:\n{metrics}"
    );
}
