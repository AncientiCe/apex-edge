//! Metrics scrape endpoint: exposes Prometheus exposition format.

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

use crate::pos::AppState;

/// Serves Prometheus scrape output. Returns 404 if no metrics handle was configured.
pub async fn serve_metrics(State(state): State<AppState>) -> Response {
    match &state.metrics_handle {
        Some(handle) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )],
            handle.render(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "metrics not configured").into_response(),
    }
}
