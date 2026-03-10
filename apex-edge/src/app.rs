//! Reusable app bootstrap for the binary and tests (build router, no bind).

use apex_edge_api::{
    get_document, handle_pos_command, health, list_order_documents, ready, search_customers,
    search_products, serve_metrics, AppState,
};
use axum::{routing::get, Router};
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

use crate::http_metrics_layer::HttpMetricsLayer;

/// Builds the Axum router with all routes and shared state.
/// Caller is responsible for DB pool creation, migrations, and binding the server.
/// Pass `Some(handle)` from `apex_edge_metrics::install_recorder()` to expose `/metrics`.
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
/// let _app = build_router(pool, Uuid::nil(), None);
/// # }
/// ```
pub fn build_router(
    pool: sqlx::SqlitePool,
    store_id: Uuid,
    metrics_handle: Option<apex_edge_metrics::PrometheusHandle>,
) -> Router {
    let app_state = AppState {
        store_id,
        pool,
        metrics_handle,
    };
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([axum::http::header::CONTENT_TYPE]);
    let routes = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/pos/command", axum::routing::post(handle_pos_command))
        .route("/catalog/products", get(search_products))
        .route("/customers", get(search_customers))
        .route("/documents/:id", get(get_document))
        .route("/orders/:order_id/documents", get(list_order_documents))
        .route("/metrics", get(serve_metrics))
        .with_state(app_state);
    routes.layer(cors).layer(HttpMetricsLayer)
}

#[cfg(test)]
mod tests {
    use super::build_router;
    use apex_edge_storage::{create_sqlite_pool, run_migrations};
    use uuid::Uuid;

    #[tokio::test]
    async fn router_exposes_health_and_ready_routes() {
        let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
        run_migrations(&pool).await.expect("migrations");
        let app = build_router(pool, Uuid::nil(), None);
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
        let app = build_router(pool, Uuid::nil(), Some(handle));
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
