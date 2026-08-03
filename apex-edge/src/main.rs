//! ApexEdge: store hub orchestrator. POS <-> ApexEdge <-> HQ.

use apex_edge::build_router;
use apex_edge_adapters_fiscal::{DeTseFiscalProvider, FiscalProvider, NoOpFiscalProvider};
use apex_edge_api::{AuthSettings, FiscalSettings};
use apex_edge_contracts::ContractVersion;
use apex_edge_outbox::run_dispatcher_loop;
use apex_edge_storage::{
    create_sqlite_pool, expire_stale_reservations, seed_demo_data, seed_inventory_from_catalog,
    set_audit_key, AuditKey,
};
use apex_edge_sync::{run_sync_ndjson, SyncEntityConfig, SyncSourceConfig};
use axum::http::HeaderValue;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const DEFAULT_SYNC_INTERVAL_SECONDS: u64 = 300;
const DEFAULT_RESERVATION_SWEEP_INTERVAL_SECONDS: u64 = 60;

fn reservation_sweep_interval_seconds() -> u64 {
    std::env::var("APEX_EDGE_RESERVATION_SWEEP_INTERVAL_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_RESERVATION_SWEEP_INTERVAL_SECONDS)
}

/// Expire reservations whose TTL has elapsed, releasing the held stock back to available.
/// Returns the number expired so the caller can log it; emits the expiry counter.
async fn sweep_stale_reservations(pool: &sqlx::SqlitePool) -> u64 {
    match expire_stale_reservations(pool, chrono::Utc::now()).await {
        Ok(0) => 0,
        Ok(n) => {
            metrics::counter!(apex_edge_metrics::INVENTORY_RESERVATIONS_EXPIRED_TOTAL, n);
            tracing::info!("Released {} stale stock reservation(s)", n);
            n
        }
        Err(e) => {
            tracing::warn!("Reservation sweep failed: {}", e);
            0
        }
    }
}

fn rand_bytes() -> [u8; 32] {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = ((nanos >> (i % 16)) as u8) ^ ((i as u8).wrapping_mul(31));
    }
    out
}

fn parse_sync_interval_seconds(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_SYNC_INTERVAL_SECONDS)
}

fn sync_interval_seconds_from_env() -> u64 {
    parse_sync_interval_seconds(
        std::env::var("APEX_EDGE_SYNC_INTERVAL_SECONDS")
            .ok()
            .as_deref(),
    )
}

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
        // Keep inventory early so stock is refreshed even if optional entities fail later.
        SyncEntityConfig {
            entity: "inventory".into(),
            path: "/sync/ndjson/inventory".into(),
        },
        SyncEntityConfig {
            entity: "coupons".into(),
            path: "/sync/ndjson/coupons".into(),
        },
        SyncEntityConfig {
            entity: "print_templates".into(),
            path: "/sync/ndjson/print_templates".into(),
        },
    ]
}

/// Pure provider selection so it's testable without mutating process-global env vars.
fn fiscal_provider_from_config(
    provider_name: Option<&str>,
    de_tse_configured: bool,
) -> std::sync::Arc<dyn FiscalProvider + Send + Sync> {
    match provider_name {
        Some("de_tse") => std::sync::Arc::new(DeTseFiscalProvider::new(de_tse_configured)),
        _ => std::sync::Arc::new(NoOpFiscalProvider),
    }
}

/// Selects the fiscal provider and currency from environment configuration.
/// Defaults to `NoOpFiscalProvider` (no fiscal signing) with currency `USD`, matching
/// deployments in non-fiscalized markets (US/CA). Set `APEX_EDGE_FISCAL_PROVIDER=de_tse`
/// plus `APEX_EDGE_FISCAL_DE_TSE_CONFIGURED=true` once TSE certification is in place;
/// leaving it unconfigured makes fiscal signing fail closed at finalize time.
fn fiscal_settings_from_env() -> FiscalSettings {
    let currency = std::env::var("APEX_EDGE_CURRENCY").unwrap_or_else(|_| "USD".into());
    let de_tse_configured = std::env::var("APEX_EDGE_FISCAL_DE_TSE_CONFIGURED")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    let provider = fiscal_provider_from_config(
        std::env::var("APEX_EDGE_FISCAL_PROVIDER").ok().as_deref(),
        de_tse_configured,
    );
    FiscalSettings { provider, currency }
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

    let audit_key_id = std::env::var("APEX_EDGE_AUDIT_KEY_ID")
        .unwrap_or_else(|_| format!("hub-{}", Uuid::new_v4()));
    let audit_secret = match std::env::var("APEX_EDGE_AUDIT_KEY_PATH").ok() {
        Some(path) => std::fs::read(&path).unwrap_or_else(|_| {
            let generated: [u8; 32] = rand_bytes();
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, generated);
            generated.to_vec()
        }),
        None => std::env::var("APEX_EDGE_AUDIT_KEY_SECRET")
            .ok()
            .map(|s| s.into_bytes())
            .unwrap_or_else(|| rand_bytes().to_vec()),
    };
    set_audit_key(AuditKey::new(audit_key_id.clone(), audit_secret));
    tracing::info!("Audit chain signing key loaded (id={})", audit_key_id);
    if std::env::args().any(|a| a == "init" || a == "--init") {
        println!("ApexEdge initialized");
        println!("database={db_path}");
        println!("audit_key_id={audit_key_id}");
        println!("admin_pairing_code_endpoint=POST /auth/pairing-codes");
        return Ok(());
    }
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

    // Seed the real-time inventory ledger from any catalog stock already present so the
    // oversell guard is active immediately. Synced inventory rebases the baseline later.
    match seed_inventory_from_catalog(&pool, Uuid::nil()).await {
        Ok(n) if n > 0 => tracing::info!("Seeded inventory ledger for {} item(s)", n),
        Ok(_) => {}
        Err(e) => tracing::warn!("Inventory ledger seeding failed: {}", e),
    }

    // Crash recovery: release any reservations stranded by a previous crash mid-cart,
    // then run a periodic sweeper so abandoned carts free their held stock on TTL.
    sweep_stale_reservations(&pool).await;
    let sweep_interval = reservation_sweep_interval_seconds();
    let pool_sweeper = pool.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(sweep_interval));
        interval.tick().await;
        loop {
            interval.tick().await;
            sweep_stale_reservations(&pool_sweeper).await;
        }
    });

    let sync_source_url = std::env::var("APEX_EDGE_SYNC_SOURCE_URL").ok();
    if let Some(ref base_url) = sync_source_url {
        let sync_interval_seconds = sync_interval_seconds_from_env();
        let config = SyncSourceConfig {
            base_url: base_url.trim_end_matches('/').to_string(),
            entities: default_sync_entities(),
        };
        tracing::info!("Running sync on startup from {}", base_url);
        run_sync_once(&pool, &config).await;
        let pool_daily = pool.clone();
        let config_daily = config.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(sync_interval_seconds));
            interval.tick().await;
            loop {
                interval.tick().await;
                tracing::info!(
                    "Running scheduled sync (interval={}s)",
                    sync_interval_seconds
                );
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
    let external_public_key_pem = std::env::var("APEX_EDGE_AUTH_EXTERNAL_PUBLIC_KEY_PEM_PATH")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok());
    let auth_settings = AuthSettings {
        enabled: std::env::var("APEX_EDGE_AUTH_ENABLED")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false),
        external_issuer: std::env::var("APEX_EDGE_AUTH_EXTERNAL_ISSUER").unwrap_or_default(),
        external_audience: std::env::var("APEX_EDGE_AUTH_EXTERNAL_AUDIENCE").unwrap_or_default(),
        external_hs256_secret: std::env::var("APEX_EDGE_AUTH_EXTERNAL_HS256_SECRET").ok(),
        external_public_key_pem,
        session_signing_secret: std::env::var("APEX_EDGE_AUTH_SESSION_SIGNING_SECRET")
            .unwrap_or_else(|_| "dev-hub-secret".into()),
        access_ttl_seconds: std::env::var("APEX_EDGE_AUTH_ACCESS_TTL_SECONDS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(300),
        refresh_ttl_seconds: std::env::var("APEX_EDGE_AUTH_REFRESH_TTL_SECONDS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(3600),
        pairing_code_ttl_seconds: std::env::var("APEX_EDGE_AUTH_PAIRING_CODE_TTL_SECONDS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(300),
        pairing_code_length: std::env::var("APEX_EDGE_AUTH_PAIRING_CODE_LENGTH")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(6),
        pairing_max_attempts: std::env::var("APEX_EDGE_AUTH_PAIRING_MAX_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(3),
    };
    let fiscal_settings = fiscal_settings_from_env();
    tracing::info!(
        "Fiscal provider: {} (currency={})",
        fiscal_settings.provider.provider_code(),
        fiscal_settings.currency
    );
    let app = build_router(
        pool,
        Uuid::nil(),
        Some(metrics_handle),
        allowed_origins,
        auth_settings,
        fiscal_settings,
    );

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("ApexEdge listening on {}", addr);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{default_sync_entities, fiscal_provider_from_config, parse_sync_interval_seconds};
    use apex_edge_adapters_fiscal::{FiscalError, FiscalReceiptRequest};
    use uuid::Uuid;

    #[test]
    fn default_entities_sync_inventory_before_optional_entities() {
        let entities = default_sync_entities();
        let inventory_pos = entities
            .iter()
            .position(|e| e.entity == "inventory")
            .expect("inventory entity should exist");
        let coupons_pos = entities
            .iter()
            .position(|e| e.entity == "coupons")
            .expect("coupons entity should exist");
        let print_templates_pos = entities
            .iter()
            .position(|e| e.entity == "print_templates")
            .expect("print_templates entity should exist");

        assert!(
            inventory_pos < coupons_pos,
            "inventory should run before coupons"
        );
        assert!(
            inventory_pos < print_templates_pos,
            "inventory should run before print_templates"
        );
    }

    #[test]
    fn sync_interval_parser_defaults_to_five_minutes() {
        assert_eq!(parse_sync_interval_seconds(None), 300);
        assert_eq!(parse_sync_interval_seconds(Some("")), 300);
        assert_eq!(parse_sync_interval_seconds(Some("0")), 300);
        assert_eq!(parse_sync_interval_seconds(Some("abc")), 300);
    }

    #[test]
    fn sync_interval_parser_accepts_positive_seconds() {
        assert_eq!(parse_sync_interval_seconds(Some("60")), 60);
        assert_eq!(parse_sync_interval_seconds(Some("900")), 900);
    }

    #[test]
    fn fiscal_provider_defaults_to_noop_when_unset() {
        let provider = fiscal_provider_from_config(None, false);
        assert_eq!(provider.provider_code(), "noop");
    }

    #[test]
    fn fiscal_provider_selects_de_tse_when_configured() {
        let provider = fiscal_provider_from_config(Some("de_tse"), true);
        assert_eq!(provider.provider_code(), "de_tse");
        let receipt = provider
            .sign_receipt(FiscalReceiptRequest {
                order_id: Uuid::new_v4(),
                total_cents: 500,
                currency: "EUR".into(),
            })
            .expect("configured de_tse should sign");
        assert!(receipt.fiscal_id.is_some());
    }

    #[test]
    fn fiscal_provider_de_tse_fails_closed_when_not_configured() {
        let provider = fiscal_provider_from_config(Some("de_tse"), false);
        let err = provider
            .sign_receipt(FiscalReceiptRequest {
                order_id: Uuid::new_v4(),
                total_cents: 500,
                currency: "EUR".into(),
            })
            .expect_err("unconfigured de_tse must fail closed");
        assert_eq!(
            err,
            FiscalError::NotConfigured {
                provider: "de_tse".into()
            }
        );
    }
}
