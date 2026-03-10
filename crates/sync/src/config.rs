//! Plug-and-play config for fetching sync data from any server (HQ, test mock, etc.).
//! Set `base_url` and one `path` per entity; the fetcher uses these to GET each endpoint.

use serde::{Deserialize, Serialize};

/// Configurable sync source: base URL + per-entity path. Use with any server (HQ, test mock, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSourceConfig {
    /// Base URL of the sync server (e.g. `https://hq.example.com`, `http://127.0.0.1:3030`).
    pub base_url: String,
    /// One entry per sync entity; path is appended to base_url (e.g. `/sync/catalog`).
    pub entities: Vec<SyncEntityConfig>,
}

/// Per-entity endpoint and optional total for progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEntityConfig {
    /// Entity name (e.g. `catalog`, `price_book`, `tax_rules`, `promotions`, `customers`).
    pub entity: String,
    /// Path for this entity (e.g. `/sync/catalog`). Request URL = base_url + path.
    pub path: String,
}

impl SyncSourceConfig {
    /// Full URL for an entity.
    pub fn url_for(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        format!("{}/{}", base, path)
    }
}
