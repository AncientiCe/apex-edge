//! Health and readiness endpoints.

use axum::{extract::State, Json};
use serde::Serialize;

use crate::pos::AppState;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
}

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
    })
}

/// Readiness: checks DB connectivity. Returns 503 if DB is unavailable.
pub async fn ready(
    State(state): State<AppState>,
) -> Result<Json<HealthResponse>, axum::http::StatusCode> {
    sqlx::query("SELECT 1")
        .execute(&state.pool)
        .await
        .map_err(|_| axum::http::StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(HealthResponse {
        status: "ready".into(),
    }))
}
