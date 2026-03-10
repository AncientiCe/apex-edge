//! Reusable app bootstrap for the binary and tests (build router, no bind).

use apex_edge_api::{
    get_document, handle_pos_command, health, list_order_documents, ready, AppState,
};
use axum::{routing::get, Router};
use uuid::Uuid;

/// Builds the Axum router with all routes and shared state.
/// Caller is responsible for DB pool creation, migrations, and binding the server.
pub fn build_router(pool: sqlx::SqlitePool, store_id: Uuid) -> Router {
    let app_state = AppState { store_id, pool };
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/pos/command", axum::routing::post(handle_pos_command))
        .route("/documents/:id", get(get_document))
        .route("/orders/:order_id/documents", get(list_order_documents))
        .with_state(app_state)
}
