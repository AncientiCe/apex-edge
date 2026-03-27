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

/// Apply a batch of payloads to the database for the given entity.
///
/// Each entity type is deserialized from its contract type and persisted to the
/// corresponding storage table. Unknown entities are silently skipped — they advance
/// checkpoints but are not stored, which is safe for forward-compatibility.
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
                let json = serde_json::to_string(&promo)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                apex_edge_storage::insert_promotion(pool, promo.id, store_id, &json)
                    .await
                    .map_err(crate::ingest::IngestError::Storage)
                    .map_err(RunSyncError::Ingest)?;
            }
        }
        "catalog" => {
            use apex_edge_contracts::CatalogItem;
            let mut items: Vec<CatalogItem> = Vec::with_capacity(batch.len());
            for payload in batch {
                let item: CatalogItem = serde_json::from_slice(payload)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                items.push(item);
            }
            apex_edge_storage::replace_catalog_items(pool, store_id, &items)
                .await
                .map_err(crate::ingest::IngestError::Storage)
                .map_err(RunSyncError::Ingest)?;
        }
        "categories" => {
            use apex_edge_contracts::Category;
            for payload in batch {
                let cat: Category = serde_json::from_slice(payload)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                apex_edge_storage::insert_category(pool, cat.id, store_id, &cat.name)
                    .await
                    .map_err(crate::ingest::IngestError::Storage)
                    .map_err(RunSyncError::Ingest)?;
            }
        }
        "price_book" => {
            use apex_edge_contracts::PriceBook;
            let mut all_entries: Vec<(uuid::Uuid, Option<uuid::Uuid>, u64, String)> = Vec::new();
            for payload in batch {
                let book: PriceBook = serde_json::from_slice(payload)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                for entry in book.entries {
                    all_entries.push((
                        entry.item_id,
                        entry.modifier_option_id,
                        entry.price_cents,
                        entry.currency,
                    ));
                }
            }
            apex_edge_storage::replace_price_book_entries(pool, store_id, &all_entries)
                .await
                .map_err(crate::ingest::IngestError::Storage)
                .map_err(RunSyncError::Ingest)?;
        }
        "tax_rules" => {
            use apex_edge_contracts::TaxRule;
            for payload in batch {
                let rule: TaxRule = serde_json::from_slice(payload)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                apex_edge_storage::insert_tax_rule(
                    pool,
                    rule.id,
                    store_id,
                    rule.tax_category_id,
                    rule.rate_bps,
                    &rule.name,
                    rule.inclusive,
                )
                .await
                .map_err(crate::ingest::IngestError::Storage)
                .map_err(RunSyncError::Ingest)?;
            }
        }
        "customers" => {
            use apex_edge_contracts::Customer;
            for payload in batch {
                let customer: Customer = serde_json::from_slice(payload)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                apex_edge_storage::insert_customer(
                    pool,
                    customer.id,
                    store_id,
                    &customer.code,
                    &customer.name,
                    customer.email.as_deref(),
                )
                .await
                .map_err(crate::ingest::IngestError::Storage)
                .map_err(RunSyncError::Ingest)?;
            }
        }
        "inventory" => {
            use apex_edge_contracts::InventoryLevel;
            let mut levels: Vec<InventoryLevel> = Vec::with_capacity(batch.len());
            for payload in batch {
                let level: InventoryLevel = serde_json::from_slice(payload)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                levels.push(level);
            }
            apex_edge_storage::replace_inventory_levels(pool, store_id, &levels)
                .await
                .map_err(crate::ingest::IngestError::Storage)
                .map_err(RunSyncError::Ingest)?;
        }
        "print_templates" => {
            use apex_edge_contracts::PrintTemplateConfig;
            for payload in batch {
                let template: PrintTemplateConfig = serde_json::from_slice(payload)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                // Use serde (rename_all = "snake_case") to get document_type string
                let doc_type_str = serde_json::to_string(&template.document_type)
                    .ok()
                    .map(|s| s.trim_matches('"').to_string())
                    .unwrap_or_else(|| "customer_receipt".into());
                apex_edge_storage::upsert_print_template(
                    pool,
                    store_id,
                    &doc_type_str,
                    template.id,
                    &template.template_body,
                    template.version as i64,
                )
                .await
                .map_err(crate::ingest::IngestError::Storage)
                .map_err(RunSyncError::Ingest)?;
            }
        }
        "coupons" => {
            use apex_edge_contracts::CouponDefinition;
            for payload in batch {
                let coupon: CouponDefinition = serde_json::from_slice(payload)
                    .map_err(|_| crate::ingest::IngestError::InvalidPayload)?;
                apex_edge_storage::upsert_coupon_definition(pool, store_id, &coupon)
                    .await
                    .map_err(crate::ingest::IngestError::Storage)
                    .map_err(RunSyncError::Ingest)?;
            }
        }
        _ => {
            // Unknown entity: checkpoint advances but no data is stored.
            // This is intentional for forward-compatibility with future HQ entity types.
            tracing::debug!(entity, "skipping unknown sync entity (no storage handler)");
        }
    }
    Ok(())
}

/// Errors from run_sync_ndjson.
#[derive(Debug, thiserror::Error)]
pub enum RunSyncError {
    #[error("fetch: {0}")]
    Fetch(#[from] FetchError),
    #[error("ingest: {0}")]
    Ingest(#[from] crate::ingest::IngestError),
}
