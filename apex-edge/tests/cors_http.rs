//! CORS: browser origin can preflight and send requests to POS/document endpoints.
//! Also validates configurable origin restriction for the internal-alpha security baseline.

use apex_edge::build_router;
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use axum::http::{HeaderValue, Method, StatusCode};
use tokio::net::TcpListener;
use uuid::Uuid;

const FRONTEND_ORIGIN: &str = "http://localhost:5173";

async fn start_app() -> u16 {
    start_app_with_origins(vec![]).await
}

async fn start_app_with_origins(allowed_origins: Vec<HeaderValue>) -> u16 {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let app = build_router(pool, Uuid::nil(), None, allowed_origins);
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

/// When specific origins are configured, a trusted origin receives CORS headers.
#[tokio::test]
async fn cors_restricted_trusted_origin_is_allowed() {
    const TRUSTED: &str = "http://trusted.local";
    let port = start_app_with_origins(vec![HeaderValue::from_static(TRUSTED)]).await;
    let client = reqwest::Client::new();
    let res = client
        .request(Method::OPTIONS, format!("http://127.0.0.1:{port}/health"))
        .header("Origin", TRUSTED)
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let allow_origin = res
        .headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        allow_origin, TRUSTED,
        "trusted origin should appear in allow-origin"
    );
}

/// When specific origins are configured, an unknown origin must NOT receive a
/// matching `access-control-allow-origin` header.
#[tokio::test]
async fn cors_restricted_unknown_origin_is_rejected() {
    const TRUSTED: &str = "http://trusted.local";
    const EVIL: &str = "http://evil.example";
    let port = start_app_with_origins(vec![HeaderValue::from_static(TRUSTED)]).await;
    let client = reqwest::Client::new();
    let res = client
        .request(Method::OPTIONS, format!("http://127.0.0.1:{port}/health"))
        .header("Origin", EVIL)
        .header("Access-Control-Request-Method", "GET")
        .send()
        .await
        .expect("request");
    let allow_origin = res
        .headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_ne!(
        allow_origin, EVIL,
        "untrusted origin must not be reflected in allow-origin"
    );
}
