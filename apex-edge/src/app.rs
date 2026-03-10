//! Reusable app bootstrap for the binary and tests (build router, no bind).

use apex_edge_api::{
    get_document, handle_pos_command, health, list_order_documents, ready, AppState,
};
use axum::{routing::get, Router};
use uuid::Uuid;

/// Builds the Axum router with all routes and shared state.
/// Caller is responsible for DB pool creation, migrations, and binding the server.
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
/// let _app = build_router(pool, Uuid::nil());
/// # }
/// ```
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

#[cfg(test)]
mod tests {
    use super::build_router;
    use apex_edge_storage::{create_sqlite_pool, run_migrations};
    use uuid::Uuid;

    #[tokio::test]
    async fn router_exposes_health_and_ready_routes() {
        let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
        run_migrations(&pool).await.expect("migrations");
        let app = build_router(pool, Uuid::nil());
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
}
