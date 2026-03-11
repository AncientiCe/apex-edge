//! ApexEdge: store hub orchestrator. POS <-> ApexEdge <-> HQ.

use apex_edge::build_router;
use apex_edge_contracts::ContractVersion;
use apex_edge_outbox::run_dispatcher_loop;
use apex_edge_storage::{create_sqlite_pool, seed_demo_data};
use apex_edge_sync::{run_sync_ndjson, SyncEntityConfig, SyncSourceConfig};
use axum::http::HeaderValue;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

/// Default NDJSON entity paths (matches example-sync-source tool).
fn default_sync_entities() -> Vec<SyncEntityConfig> {
    vec![
        SyncEntityConfig {
            entity: "catalog".into(),
            path: "/sync/ndjson/catalog".into(),
        },
        SyncEntityConfig {
            entity: "categories".into(),
            path: "/sync/ndjson/categories".into(),
        },
        SyncEntityConfig {
            entity: "price_book".into(),
            path: "/sync/ndjson/price_book".into(),
        },
        SyncEntityConfig {
            entity: "tax_rules".into(),
            path: "/sync/ndjson/tax_rules".into(),
        },
        SyncEntityConfig {
            entity: "promotions".into(),
            path: "/sync/ndjson/promotions".into(),
        },
        SyncEntityConfig {
            entity: "customers".into(),
            path: "/sync/ndjson/customers".into(),
        },
        SyncEntityConfig {
            entity: "coupons".into(),
            path: "/sync/ndjson/coupons".into(),
        },
        SyncEntityConfig {
            entity: "inventory".into(),
            path: "/sync/ndjson/inventory".into(),
        },
    ]
}

/// Run one sync cycle; log outcome. Caller ensures config is some.
async fn run_sync_once(pool: &sqlx::SqlitePool, config: &SyncSourceConfig) {
    let client = reqwest::Client::new();
    match run_sync_ndjson(&client, pool, config, ContractVersion::V1_0_0, Uuid::nil()).await {
        Ok(()) => tracing::info!("Sync completed successfully"),
        Err(e) => tracing::warn!("Sync failed: {}", e),
    }
}

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

    let sync_source_url = std::env::var("APEX_EDGE_SYNC_SOURCE_URL").ok();
    if let Some(ref base_url) = sync_source_url {
        let config = SyncSourceConfig {
            base_url: base_url.trim_end_matches('/').to_string(),
            entities: default_sync_entities(),
        };
        tracing::info!("Running sync on startup from {}", base_url);
        run_sync_once(&pool, &config).await;
        let pool_daily = pool.clone();
        let config_daily = config.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
            interval.tick().await;
            loop {
                interval.tick().await;
                tracing::info!("Running scheduled daily sync");
                run_sync_once(&pool_daily, &config_daily).await;
            }
        });
    }

    let hq_submit_url = std::env::var("APEX_EDGE_HQ_SUBMIT_URL").ok();
    if let Some(ref url) = hq_submit_url {
        let pool_dispatch = pool.clone();
        let url_dispatch = url.clone();
        tokio::spawn(async move {
            run_dispatcher_loop(
                pool_dispatch,
                reqwest::Client::new(),
                url_dispatch,
                std::time::Duration::from_secs(30),
            )
            .await;
        });
        tracing::info!("Outbox dispatcher started (HQ submit URL: {})", url);
    }

    let allowed_origins: Vec<HeaderValue> = std::env::var("APEX_EDGE_ALLOWED_ORIGINS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<HeaderValue>().ok())
        .collect();
    if !allowed_origins.is_empty() {
        tracing::info!("CORS restricted to {} origin(s)", allowed_origins.len());
    } else {
        tracing::warn!("CORS: allowing all origins (set APEX_EDGE_ALLOWED_ORIGINS for production)");
    }
    let metrics_handle = apex_edge_metrics::install_recorder()?;
    let app = build_router(pool, Uuid::nil(), Some(metrics_handle), allowed_origins);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("ApexEdge listening on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}
