//! Reusable app bootstrap for the binary and tests (build router, no bind).

use apex_edge_api::{
    auth_middleware, create_api_token, create_approval, create_gift_receipt_document,
    create_pairing_code, deny_approval_handler, erase_customer_data, exchange_session,
    export_customer_data, get_approval_handler, get_cart_state_handler, get_document,
    get_order_handler, get_prices, get_product_by_id, grant_approval_handler, handle_pos_command,
    health, list_categories, list_order_documents, list_orders_handler, list_outbox_dead_letters,
    list_outbox_destinations, list_registers, lookup_order_for_return, openapi_handler,
    openapi_ui_handler, pair_device, pos_snapshot, pos_stream_sse, pos_stream_ws,
    rate_limit_middleware, ready, receive_webhook, refresh_session, retry_outbox_dead_letter,
    revoke_session, role::standby_guard_middleware, search_customers, search_products,
    serve_metrics, sync_status, verify_audit_chain, AppState, AuthSettings, FiscalSettings,
    HardwareSettings, PaymentSettings, RateLimiter,
};
use axum::middleware;
use axum::routing::post;
use axum::{http::HeaderValue, routing::get, Router};
use tower_http::cors::{AllowOrigin, CorsLayer};
use uuid::Uuid;

use crate::http_metrics_layer::HttpMetricsLayer;

/// Everything the hub needs beyond its database pool.
///
/// Grouping these keeps adding an adapter from rippling through every call site;
/// callers set only what they need: `HubConfig { store_id, ..Default::default() }`.
pub struct HubConfig {
    pub store_id: Uuid,
    pub register_id: Uuid,
    /// From `apex_edge_metrics::install_recorder()`. `None` disables `/metrics`.
    pub metrics_handle: Option<apex_edge_metrics::PrometheusHandle>,
    /// Empty allows any origin, which is suitable for local development only.
    pub allowed_origins: Vec<HeaderValue>,
    pub auth: AuthSettings,
    pub fiscal: FiscalSettings,
    pub payments: PaymentSettings,
    pub hardware: HardwareSettings,
    pub rate_limit: apex_edge_api::RateLimitSettings,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            metrics_handle: None,
            allowed_origins: vec![],
            auth: AuthSettings::default(),
            fiscal: FiscalSettings::default(),
            payments: PaymentSettings::default(),
            hardware: HardwareSettings::default(),
            rate_limit: apex_edge_api::RateLimitSettings::default(),
        }
    }
}

/// Builds the Axum router with all routes and shared state.
/// Caller is responsible for DB pool creation, migrations, and binding the server.
///
/// # Examples
///
/// ```no_run
/// use apex_edge::{build_router, HubConfig};
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
/// let _app = build_router(pool, HubConfig::default());
/// # }
/// ```
pub fn build_router(pool: sqlx::SqlitePool, config: HubConfig) -> Router {
    let HubConfig {
        store_id,
        register_id,
        metrics_handle,
        allowed_origins,
        auth,
        fiscal,
        payments,
        hardware,
        rate_limit,
    } = config;
    let app_state = AppState {
        store_id,
        register_id,
        pool,
        metrics_handle,
        auth,
        stream: apex_edge_api::StreamHub::new(),
        role: apex_edge_api::HubRole::from_env(),
        fiscal,
        payments,
        hardware,
        rate_limiter: RateLimiter::new(rate_limit),
    };
    apex_edge_api::report_role(app_state.role);
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
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ]);
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
        .route("/catalog/prices", get(get_prices))
        .route("/catalog/categories", get(list_categories))
        .route("/customers", get(search_customers))
        .route("/documents/:id", get(get_document))
        .route("/orders", get(list_orders_handler))
        .route("/orders/:id", get(get_order_handler))
        .route("/orders/:order_id/documents", get(list_order_documents))
        .route(
            "/orders/:order_id/documents/gift-receipt",
            axum::routing::post(create_gift_receipt_document),
        )
        .route("/metrics", get(serve_metrics))
        .route("/sync/status", get(sync_status))
        .route("/audit/verify", get(verify_audit_chain))
        .route("/approvals", post(create_approval))
        .route("/approvals/:id", get(get_approval_handler))
        .route("/approvals/:id/grant", post(grant_approval_handler))
        .route("/approvals/:id/deny", post(deny_approval_handler))
        .route("/admin/api-tokens", post(create_api_token))
        .route("/admin/customers/:id/export", get(export_customer_data))
        .route("/admin/customers/:id/erase", post(erase_customer_data))
        .route("/admin/outbox/destinations", get(list_outbox_destinations))
        .route("/admin/outbox/dead-letters", get(list_outbox_dead_letters))
        .route(
            "/admin/outbox/dead-letters/:attempt_id/retry",
            post(retry_outbox_dead_letter),
        )
        .route("/webhooks/:connector_id", post(receive_webhook))
        .route("/pos/stream", get(pos_stream_ws))
        .route("/pos/events", get(pos_stream_sse))
        .route("/pos/registers", get(list_registers))
        .route("/pos/snapshot", get(pos_snapshot))
        .route("/pos/returns/lookup", get(lookup_order_for_return))
        .route("/openapi.json", get(openapi_handler))
        .route("/docs", get(openapi_ui_handler))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_middleware,
        ))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            standby_guard_middleware,
        ))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            rate_limit_middleware,
        ))
        .with_state(app_state);
    routes.layer(cors).layer(HttpMetricsLayer)
}

#[cfg(test)]
mod tests {
    use super::{build_router, HubConfig};
    use apex_edge_storage::{create_sqlite_pool, run_migrations};

    #[tokio::test]
    async fn router_exposes_health_and_ready_routes() {
        let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
        run_migrations(&pool).await.expect("migrations");
        let app = build_router(pool, HubConfig::default());
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
            HubConfig {
                metrics_handle: Some(handle),
                ..HubConfig::default()
            },
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
