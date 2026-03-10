//! ApexEdge: store hub orchestrator. POS <-> ApexEdge <-> HQ.

use apex_edge::build_router;
use apex_edge_storage::create_sqlite_pool;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("apex_edge=info".parse()?))
        .init();

    let db_path = std::env::var("APEX_EDGE_DB").unwrap_or_else(|_| "apex_edge.db".into());
    let pool = create_sqlite_pool(&db_path).await?;
    apex_edge_storage::run_migrations(&pool).await?;

    let app = build_router(pool, Uuid::nil());

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("ApexEdge listening on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}
