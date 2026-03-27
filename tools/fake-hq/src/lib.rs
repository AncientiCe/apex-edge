pub mod routes;
pub mod storage;

use std::sync::Arc;

use axum::{routing::get, Router};
use metrics_exporter_prometheus::PrometheusHandle;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<storage::Storage>,
    pub metrics_handle: Option<PrometheusHandle>,
}

pub fn build_app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(routes::ui::health))
        .route("/metrics", get(metrics))
        .merge(routes::orders::router())
        .merge(routes::sync::router())
        .merge(routes::ui::router())
        .with_state(state)
}

async fn metrics(axum::extract::State(state): axum::extract::State<Arc<AppState>>) -> String {
    state
        .metrics_handle
        .as_ref()
        .map(PrometheusHandle::render)
        .unwrap_or_default()
}
