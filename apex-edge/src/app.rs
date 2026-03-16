//! Reusable app bootstrap for the binary and tests (build router, no bind).

use apex_edge_api::{
    auth_middleware, create_gift_receipt_document, create_pairing_code, exchange_session,
    get_cart_state_handler, get_document, get_product_by_id, handle_pos_command, health,
    list_categories, list_order_documents, pair_device, ready, refresh_session, revoke_session,
    search_customers, search_products, serve_metrics, sync_status, AppState, AuthSettings,
};
use axum::middleware;
use axum::routing::post;
use axum::{http::HeaderValue, routing::get, Router};
use tower_http::cors::{AllowOrigin, CorsLayer};
use uuid::Uuid;

use crate::http_metrics_layer::HttpMetricsLayer;

/// Builds the Axum router with all routes and shared state.
/// Caller is responsible for DB pool creation, migrations, and binding the server.
/// Pass `Some(handle)` from `apex_edge_metrics::install_recorder()` to expose `/metrics`.
/// Pass a non-empty `allowed_origins` to restrict CORS to specific origins; an empty
/// list allows all origins (wildcard — suitable for local dev only).
///
/// # Examples
///
/// ```no_run
/// use apex_edge::build_router;
/// use sqlx::sqlite::SqlitePoolOptions;
/// use uuid::Uuid;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let pool = SqlitePoolOptions::new()
///     .max_connections(1)
///     .connect("sqlite::memory:")
///     .await
///     .unwrap();
/// let _app = build_router(
///     pool,
///     Uuid::nil(),
///     None,
///     vec![],
///     apex_edge_api::AuthSettings::default(),
/// );
/// # }
/// ```
pub fn build_router(
    pool: sqlx::SqlitePool,
    store_id: Uuid,
    metrics_handle: Option<apex_edge_metrics::PrometheusHandle>,
    allowed_origins: Vec<HeaderValue>,
    auth: AuthSettings,
) -> Router {
    let app_state = AppState {
        store_id,
        pool,
        metrics_handle,
        auth,
    };
    let cors_origin = if allowed_origins.is_empty() {
        AllowOrigin::any()
    } else {
        AllowOrigin::list(allowed_origins)
    };
    let cors = CorsLayer::new()
        .allow_origin(cors_origin)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([axum::http::header::CONTENT_TYPE]);
    let routes = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/auth/pairing-codes", post(create_pairing_code))
        .route("/auth/devices/pair", post(pair_device))
        .route("/auth/sessions/exchange", post(exchange_session))
        .route("/auth/sessions/refresh", post(refresh_session))
        .route("/auth/sessions/revoke", post(revoke_session))
        .route("/pos/command", axum::routing::post(handle_pos_command))
        .route("/pos/cart/:cart_id", get(get_cart_state_handler))
        .route("/catalog/products", get(search_products))
        .route("/catalog/products/:id", get(get_product_by_id))
        .route("/catalog/categories", get(list_categories))
        .route("/customers", get(search_customers))
        .route("/documents/:id", get(get_document))
        .route("/orders/:order_id/documents", get(list_order_documents))
        .route(
            "/orders/:order_id/documents/gift-receipt",
            axum::routing::post(create_gift_receipt_document),
        )
        .route("/metrics", get(serve_metrics))
        .route("/sync/status", get(sync_status))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_middleware,
        ))
        .with_state(app_state);
    routes.layer(cors).layer(HttpMetricsLayer)
}

#[cfg(test)]
mod tests {
    use super::build_router;
    use apex_edge_api::AuthSettings;
    use apex_edge_storage::{create_sqlite_pool, run_migrations};
    use uuid::Uuid;

    #[tokio::test]
    async fn router_exposes_health_and_ready_routes() {
        let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
        run_migrations(&pool).await.expect("migrations");
        let app = build_router(pool, Uuid::nil(), None, vec![], AuthSettings::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        let health = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .expect("health");
        assert_eq!(health.status(), axum::http::StatusCode::OK);

        let ready = client
            .get(format!("http://127.0.0.1:{port}/ready"))
            .send()
            .await
            .expect("ready");
        assert_eq!(ready.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_prometheus_exposition_when_recorder_installed() {
        let handle = apex_edge_metrics::install_recorder().expect("install recorder");
        let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
        run_migrations(&pool).await.expect("migrations");
        let app = build_router(
            pool,
            Uuid::nil(),
            Some(handle),
            vec![],
            AuthSettings::default(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let client = reqwest::Client::new();
        // Trigger a request so the HTTP metrics layer records at least one metric
        let _ = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await;
        let resp = client
            .get(format!("http://127.0.0.1:{port}/metrics"))
            .send()
            .await
            .expect("metrics request");
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert!(
            resp.headers()
                .get("content-type")
                .map(|v| v.to_str().unwrap_or("").contains("text/plain"))
                .unwrap_or(false),
            "metrics endpoint should return text/plain"
        );
        let body = resp.text().await.expect("metrics body");
        // With at least one request above, exposition typically includes apex_edge_* metrics
        assert!(
            body.is_empty()
                || body.contains("apex_edge")
                || body.contains("# HELP")
                || body.contains("# TYPE"),
            "metrics body should be Prometheus exposition; len={}",
            body.len()
        );
    }
}
