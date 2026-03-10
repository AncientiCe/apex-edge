//! CORS: browser origin can preflight and send requests to POS/document endpoints.

use apex_edge::build_router;
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use axum::http::{Method, StatusCode};
use tokio::net::TcpListener;
use uuid::Uuid;

const FRONTEND_ORIGIN: &str = "http://localhost:5173";

async fn start_app() -> u16 {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let app = build_router(pool, Uuid::nil(), None);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    port
}

#[tokio::test]
async fn cors_preflight_pos_command_returns_allowed_origin_and_methods() {
    let port = start_app().await;
    let client = reqwest::Client::new();
    let res = client
        .request(
            Method::OPTIONS,
            format!("http://127.0.0.1:{port}/pos/command"),
        )
        .header("Origin", FRONTEND_ORIGIN)
        .header("Access-Control-Request-Method", "POST")
        .header("Access-Control-Request-Headers", "content-type")
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let allow_origin = res
        .headers()
        .get("access-control-allow-origin")
        .expect("access-control-allow-origin header");
    let s = allow_origin.to_str().unwrap_or("");
    assert!(
        s == "*" || s == FRONTEND_ORIGIN,
        "allow-origin must be * or frontend origin, got {:?}",
        s
    );
    let allow_methods = res
        .headers()
        .get("access-control-allow-methods")
        .expect("access-control-allow-methods header");
    assert!(
        allow_methods.to_str().unwrap_or("").contains("POST"),
        "allow-methods should include POST"
    );
}

#[tokio::test]
async fn cors_actual_request_returns_allow_origin() {
    let port = start_app().await;
    let client = reqwest::Client::new();
    let res = client
        .get(format!("http://127.0.0.1:{port}/health"))
        .header("Origin", FRONTEND_ORIGIN)
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let allow_origin = res.headers().get("access-control-allow-origin");
    let s = allow_origin.and_then(|v| v.to_str().ok());
    assert!(
        s == Some("*") || s == Some(FRONTEND_ORIGIN),
        "allow-origin must be * or frontend origin, got {:?}",
        s
    );
}
