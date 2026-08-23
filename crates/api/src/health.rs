//! Health and readiness endpoints.

use axum::{extract::State, Json};
use serde::Serialize;

use crate::pos::AppState;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Serialize)]
pub struct ReadyResponse {
    pub status: String,
    pub store_id: uuid::Uuid,
    pub register_id: uuid::Uuid,
}

/// Liveness endpoint payload.
///
/// # Examples
///
/// ```no_run
/// use apex_edge_api::health;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let json = health().await;
/// assert_eq!(json.0.status, "ok");
/// # }
/// ```
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
    })
}

/// Readiness: checks DB connectivity. Returns 503 if DB is unavailable.
pub async fn ready(
    State(state): State<AppState>,
) -> Result<Json<ReadyResponse>, axum::http::StatusCode> {
    sqlx::query("SELECT 1")
        .execute(&state.pool)
        .await
        .map_err(|_| axum::http::StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(ReadyResponse {
        status: "ready".into(),
        store_id: state.store_id,
        register_id: state.register_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::{health, ready};
    use crate::pos::AppState;
    use axum::extract::State;
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    #[tokio::test]
    async fn health_returns_ok() {
        let h = health().await;
        assert_eq!(h.0.status, "ok");
    }

    #[tokio::test]
    async fn ready_returns_ready_with_live_db() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("pool");
        let state = AppState::new(pool, Uuid::nil());
        let r = ready(State(state)).await.expect("ready endpoint");
        assert_eq!(r.0.status, "ready");
    }
}
