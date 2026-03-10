//! Async sync workers: HQ -> ApexEdge data ingest with checkpoints and conflict policy.
//! Plug-and-play config for fetching from any sync server; progress % for any completion level.

pub mod config;
pub mod conflict;
pub mod fetch;
pub mod ingest;
pub mod progress;

pub use config::*;
pub use conflict::*;
pub use fetch::*;
pub use ingest::*;
pub use progress::*;

/// Run full sync using NDJSON streaming per entity; streams line-by-line, collects per entity, applies to DB, then advances checkpoint.
/// Updates latest sync run and per-entity status in storage for the status page.
/// `store_id` is used when writing promotions (and other store-scoped entities) to the database.
pub async fn run_sync_ndjson(
    client: &reqwest::Client,
    pool: &sqlx::SqlitePool,
    config: &SyncSourceConfig,
    version: apex_edge_contracts::ContractVersion,
    store_id: uuid::Uuid,
) -> Result<(), RunSyncError> {
    let started = chrono::Utc::now();
    let _ =
        apex_edge_storage::upsert_latest_sync_run(pool, "running", Some(started), None, None).await;

    let result = run_sync_ndjson_inner(client, pool, config, version, store_id).await;

    let finished = chrono::Utc::now();
    match &result {
        Ok(()) => {
            let _ = apex_edge_storage::upsert_latest_sync_run(
                pool,
                "success",
                Some(started),
                Some(finished),
                None,
            )
            .await;
        }
        Err(e) => {
            let _ = apex_edge_storage::upsert_latest_sync_run(
                pool,
                "failed",
                Some(started),
                Some(finished),
                Some(&e.to_string()),
            )
            .await;
        }
    }
    result
}

async fn run_sync_ndjson_inner(
    client: &reqwest::Client,
    pool: &sqlx::SqlitePool,
    config: &SyncSourceConfig,
    version: apex_edge_contracts::ContractVersion,
    store_id: uuid::Uuid,
) -> Result<(), RunSyncError> {
    for ent in &config.entities {
        let url = config.url_for(&ent.path);
        let entity = ent.entity.clone();
        let mut batch: Vec<Vec<u8>> = Vec::new();
        let mut total: u64 = 0;
        fetch_entity_ndjson_stream(client, &url, 0, |payloads, t| {
            batch.extend(payloads.to_vec());
            total = t;
        })
        .await
        .map_err(RunSyncError::Fetch)?;

        let now = chrono::Utc::now();
        let current = batch.len() as u64;
        let percent = if total > 0 {
            Some((current as f64 / total as f64).min(1.0) * 100.0)
        } else {
            None
        };
        let _ = apex_edge_storage::upsert_entity_sync_status(
            pool,
            &entity,
            current,
            Some(total),
            percent,
            now,
            "done",
        )
        .await;

        if !batch.is_empty() {
            apply_entity_batch(pool, &entity, &batch, store_id).await?;
            ingest_batch(pool, &entity, version, &batch, ConflictPolicy::HqWins)
                .await
                .map_err(RunSyncError::Ingest)?;
        }
    }
    Ok(())
}

/// Apply a batch of payloads to the database for the given entity (e.g. insert into promotions table).
async fn apply_entity_batch(
    pool: &sqlx::SqlitePool,
    entity: &str,
    batch: &[Vec<u8>],
    store_id: uuid::Uuid,
) -> Result<(), RunSyncError> {
    match entity {
        "promotions" => {
            use apex_edge_contracts::Promotion;
            for payload in batch {
                let promo: Promotion = serde_json::from_slice(payload)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                let json = serde_json::to_string(&promo).map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                apex_edge_storage::insert_promotion(pool, promo.id, store_id, &json)
                    .await
                    .map_err(crate::ingest::IngestError::Storage)
                    .map_err(RunSyncError::Ingest)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Errors from run_sync_ndjson.
#[derive(Debug, thiserror::Error)]
pub enum RunSyncError {
    #[error("not implemented")]
    NotImplemented,
    #[error("fetch: {0}")]
    Fetch(#[from] FetchError),
    #[error("ingest: {0}")]
    Ingest(#[from] crate::ingest::IngestError),
}
