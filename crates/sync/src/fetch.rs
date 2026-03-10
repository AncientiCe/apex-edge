//! Fetch sync data from a configured server (one endpoint per entity).
//!
//! Endpoint contract: GET returns JSON `{ "items": [ "<base64 payload>", ... ], "total": N }`.
//! Decoded `items` are passed to `ingest_batch`; `total` is used for progress %.

use base64::Engine;
use serde::Deserialize;
use thiserror::Error;

use crate::config::SyncSourceConfig;
use crate::progress::{SyncEntityProgress, SyncProgressSummary};

#[derive(Error, Debug)]
pub enum FetchError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("decode: {0}")]
    Decode(String),
}

/// Response shape from a sync endpoint: items as base64 payloads and total count for progress.
#[derive(Debug, Deserialize)]
pub struct SyncEndpointResponse {
    pub items: Vec<String>,
    pub total: u64,
}

/// Fetch one entity from its configured path. Returns payloads (decoded from base64) and total for progress.
pub async fn fetch_entity(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    _since: i64,
) -> Result<(Vec<Vec<u8>>, u64), FetchError> {
    let url = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let resp = client.get(&url).send().await?;
    resp.error_for_status_ref()?;
    let body: SyncEndpointResponse = resp.json().await?;
    let engine = base64::engine::general_purpose::STANDARD;
    let payloads: Result<Vec<Vec<u8>>, _> = body
        .items
        .iter()
        .map(|s| {
            engine
                .decode(s)
                .map_err(|e| FetchError::Decode(e.to_string()))
        })
        .collect();
    Ok((payloads?, body.total))
}

/// Fetch all entities from config and return payloads per entity plus progress summary.
/// Current checkpoints from pool are used to build progress; total comes from each response.
pub async fn fetch_all(
    client: &reqwest::Client,
    config: &SyncSourceConfig,
    current_by_entity: impl Fn(&str) -> Option<i64>,
) -> Result<(Vec<(String, Vec<Vec<u8>>, u64)>, SyncProgressSummary), FetchError> {
    let mut results = Vec::with_capacity(config.entities.len());
    let mut progress_entities = Vec::with_capacity(config.entities.len());

    for ent in &config.entities {
        let (payloads, total) = fetch_entity(
            client,
            &config.base_url,
            &ent.path,
            current_by_entity(&ent.entity).unwrap_or(0),
        )
        .await?;
        let current = current_by_entity(&ent.entity).unwrap_or(0) as u64 + payloads.len() as u64;
        progress_entities.push(SyncEntityProgress {
            entity: ent.entity.clone(),
            current,
            total: Some(total),
        });
        results.push((ent.entity.clone(), payloads, total));
    }

    let summary = SyncProgressSummary::from_entities(progress_entities);
    Ok((results, summary))
}
