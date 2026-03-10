//! ApexEdge: store hub orchestrator. POS <-> ApexEdge <-> HQ.

use apex_edge::build_router;
use apex_edge_storage::{create_sqlite_pool, seed_demo_data};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("apex_edge=info".parse()?))
        .init();

    let db_path = std::env::var("APEX_EDGE_DB").unwrap_or_else(|_| {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.join("apex_edge.db").to_str().map(String::from))
            .unwrap_or_else(|| "apex_edge.db".into())
    });
    let pool = create_sqlite_pool(&db_path).await?;
    apex_edge_storage::run_migrations(&pool).await?;
    let seed_flag = std::env::args().any(|a| a == "--seed-demo")
        || std::env::var("APEX_EDGE_SEED_DEMO")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
    if seed_flag {
        let summary = seed_demo_data(&pool, Uuid::nil()).await?;
        tracing::info!(
            "Seeded demo data: categories={}, products={}, customers={}, promotions={}",
            summary.categories,
            summary.products,
            summary.customers,
            summary.promotions
        );
    }

    let metrics_handle = apex_edge_metrics::install_recorder()?;
    let app = build_router(pool, Uuid::nil(), Some(metrics_handle));

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("ApexEdge listening on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}
