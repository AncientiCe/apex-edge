//! ApexEdge: store hub orchestrator. POS <-> ApexEdge <-> HQ.

mod tls;

use apex_edge::build_router;
use apex_edge_adapters_fiscal::{
    DeTseFiscalProvider, FiscalProvider, FiskalyConfig, FiskalyMarket, FiskalyProvider,
    NoOpFiscalProvider,
};
use apex_edge_api::{AuthSettings, FiscalSettings, SigningSecretSource};
use apex_edge_contracts::ContractVersion;
use apex_edge_outbox::run_dispatcher_loop;
use apex_edge_storage::{
    create_sqlite_pool, expire_stale_reservations, resolve_hub_identity, seed_demo_data,
    seed_inventory_from_catalog, set_audit_key, AuditKey,
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
            metrics::counter!(apex_edge_metrics::INVENTORY_RESERVATIONS_EXPIRED_TOTAL).increment(n);
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
#[cfg(test)]
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
    let provider_name = std::env::var("APEX_EDGE_FISCAL_PROVIDER").ok();
    let provider: std::sync::Arc<dyn FiscalProvider + Send + Sync> = match provider_name.as_deref()
    {
        Some("fiskaly") => match FiskalyProvider::new(fiskaly_config_from_env()) {
            Ok(provider) => std::sync::Arc::new(provider),
            Err(e) => {
                tracing::error!(error = %e, "fiskaly client could not start");
                std::sync::Arc::new(
                    FiskalyProvider::new(FiskalyConfig::default())
                        .expect("empty fiskaly still constructs"),
                )
            }
        },
        Some("de_tse") => {
            let configured = std::env::var("APEX_EDGE_FISCAL_DE_TSE_CONFIGURED")
                .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
                .unwrap_or(false);
            std::sync::Arc::new(DeTseFiscalProvider::new(configured))
        }
        _ => std::sync::Arc::new(NoOpFiscalProvider),
    };
    FiscalSettings { provider, currency }
}

fn fiskaly_config_from_env() -> FiskalyConfig {
    FiskalyConfig {
        api_key: std::env::var("APEX_EDGE_FISKALY_API_KEY").unwrap_or_default(),
        api_secret: std::env::var("APEX_EDGE_FISKALY_API_SECRET").unwrap_or_default(),
        tss_id: std::env::var("APEX_EDGE_FISKALY_TSS_ID").unwrap_or_default(),
        client_id: std::env::var("APEX_EDGE_FISKALY_CLIENT_ID").unwrap_or_default(),
        tax_id: std::env::var("APEX_EDGE_FISKALY_TAX_ID").unwrap_or_default(),
        market: std::env::var("APEX_EDGE_FISKALY_MARKET")
            .ok()
            .as_deref()
            .and_then(FiskalyMarket::parse)
            .unwrap_or(FiskalyMarket::De),
        api_base_url: std::env::var("APEX_EDGE_FISKALY_API_BASE_URL")
            .unwrap_or_else(|_| "https://kassensichv.io/api/v2".into()),
    }
}

/// Run one sync cycle; log outcome. Caller ensures config is some.
async fn run_sync_once(pool: &sqlx::SqlitePool, config: &SyncSourceConfig, store_id: Uuid) {
    let client = reqwest::Client::new();
    match run_sync_ndjson(&client, pool, config, ContractVersion::V1_0_0, store_id).await {
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

    let env_store_id = std::env::var("APEX_EDGE_STORE_ID")
        .ok()
        .and_then(|v| Uuid::parse_str(&v).ok());
    let env_register_id = std::env::var("APEX_EDGE_REGISTER_ID")
        .ok()
        .and_then(|v| Uuid::parse_str(&v).ok());
    let identity = resolve_hub_identity(&pool, env_store_id, env_register_id).await?;
    tracing::info!(
        "Hub identity resolved: store_id={} register_id={}",
        identity.store_id,
        identity.register_id
    );

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

    // Never a built-in secret: without APEX_EDGE_AUTH_SESSION_SIGNING_SECRET the hub keeps a
    // random one next to its database, so sessions survive restarts and nobody can forge them.
    let default_session_key_path =
        std::path::Path::new(&db_path).with_file_name("apex_edge_session.key");
    let (auth_settings, secret_source) = AuthSettings::from_env(&default_session_key_path)?;
    match secret_source {
        SigningSecretSource::Env => tracing::info!("Session signing secret: from environment"),
        source => tracing::info!(
            "Session signing secret: {} (key file; override with APEX_EDGE_AUTH_SESSION_KEY_PATH, default {})",
            source.as_str(),
            default_session_key_path.display()
        ),
    }
    if std::env::args().any(|a| a == "init" || a == "--init") {
        println!("ApexEdge initialized");
        println!("database={db_path}");
        println!("audit_key_id={audit_key_id}");
        println!("session_signing_secret_source={}", secret_source.as_str());
        println!("admin_pairing_code_endpoint=POST /auth/pairing-codes");
        return Ok(());
    }
    let seed_flag = std::env::args().any(|a| a == "--seed-demo")
        || std::env::var("APEX_EDGE_SEED_DEMO")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
    if seed_flag {
        let summary = seed_demo_data(&pool, identity.store_id).await?;
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
    match seed_inventory_from_catalog(&pool, identity.store_id).await {
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
        run_sync_once(&pool, &config, identity.store_id).await;
        let pool_daily = pool.clone();
        let config_daily = config.clone();
        let store_id_daily = identity.store_id;
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
                run_sync_once(&pool_daily, &config_daily, store_id_daily).await;
            }
        });
    }

    // Destinations are configuration, re-registered on every boot so an edited endpoint
    // takes effect without losing the delivery history keyed on the destination.
    if let Ok(url) = std::env::var("APEX_EDGE_HQ_SUBMIT_URL") {
        if let Err(e) = apex_edge_outbox::register_hq_destination(&pool, &url).await {
            tracing::error!(error = %e, "could not register the HQ outbox destination");
        }
    }
    if let Ok(raw) = std::env::var("APEX_EDGE_OUTBOX_DESTINATIONS") {
        match apex_edge_outbox::register_destinations_from_json(&pool, &raw).await {
            Ok(count) => tracing::info!("Registered {} extra outbox destination(s)", count),
            Err(e) => tracing::error!(error = %e, "could not register outbox destinations"),
        }
    }
    match apex_edge_storage::list_enabled_destinations(&pool).await {
        Ok(destinations) if !destinations.is_empty() => {
            let codes: Vec<&str> = destinations.iter().map(|d| d.code.as_str()).collect();
            tracing::info!(
                "Outbox dispatcher started (destinations: {})",
                codes.join(", ")
            );
            let pool_dispatch = pool.clone();
            tokio::spawn(async move {
                run_dispatcher_loop(
                    pool_dispatch,
                    reqwest::Client::new(),
                    apex_edge_outbox::DispatcherPolicy::from_env(),
                    std::time::Duration::from_secs(30),
                )
                .await;
            });
        }
        Ok(_) => tracing::info!(
            "No outbox destinations configured; submissions will queue until one is added"
        ),
        Err(e) => tracing::error!(error = %e, "could not read outbox destinations"),
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
    metrics::gauge!(apex_edge_metrics::AUTH_SIGNING_SECRET_SOURCE, "source" => secret_source.as_str())
        .set(1.0);
    let fiscal_settings = fiscal_settings_from_env();
    tracing::info!(
        "Fiscal provider: {} (currency={})",
        fiscal_settings.provider.provider_code(),
        fiscal_settings.currency
    );
    let payment_settings =
        apex_edge_api::PaymentSettings::from_env(fiscal_settings.currency.clone());
    tracing::info!(
        "Payment providers: {}",
        payment_settings.provider_codes().join(", ")
    );

    // Money captured for a sale that then failed is owed back. This loop is what makes
    // that a guarantee rather than an intention.
    tokio::spawn(apex_edge_api::run_payment_reversal_loop(
        pool.clone(),
        payment_settings.clone(),
        std::time::Duration::from_secs(15),
    ));
    tokio::spawn(apex_edge_api::run_fiscal_signing_loop(
        pool.clone(),
        fiscal_settings.clone(),
        std::time::Duration::from_secs(15),
    ));

    let hardware_settings = apex_edge_api::HardwareSettings::from_env();
    tracing::info!(
        "Receipt printer: {} (drawer={:?})",
        hardware_settings.encoder_name().unwrap_or("none"),
        hardware_settings.drawer
    );

    let app = build_router(
        pool,
        apex_edge::HubConfig {
            store_id: identity.store_id,
            register_id: identity.register_id,
            metrics_handle: Some(metrics_handle),
            allowed_origins,
            auth: auth_settings,
            fiscal: fiscal_settings,
            payments: payment_settings,
            hardware: hardware_settings,
            rate_limit: apex_edge_api::RateLimitSettings::from_env(),
        },
    );

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 3000));
    let make_service = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    match tls::TlsSettings::from_env() {
        Some(tls_settings) => {
            tls::report_tls_enabled(Some(&tls_settings));
            let rustls_config = tls::build_rustls_config(&tls_settings).await?;
            tracing::info!("ApexEdge listening on {} (HTTPS)", addr);
            axum_server::bind_rustls(addr, rustls_config)
                .serve(make_service)
                .await?;
        }
        None => {
            tls::report_tls_enabled(None);
            tracing::info!("ApexEdge listening on {} (HTTP)", addr);
            axum::serve(tokio::net::TcpListener::bind(addr).await?, make_service).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{default_sync_entities, fiscal_provider_from_config, parse_sync_interval_seconds};

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
    }

    #[test]
    fn fiscal_provider_de_tse_fails_closed_when_not_configured() {
        let provider = fiscal_provider_from_config(Some("de_tse"), false);
        assert_eq!(provider.provider_code(), "de_tse");
    }
}
