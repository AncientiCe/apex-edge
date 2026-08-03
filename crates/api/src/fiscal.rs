//! Fiscal provider selection, wired into `AppState` and called from order finalize.
//!
//! The `apex-edge-adapters-fiscal` crate ships the actual signing logic (NoOp, DE-TSE);
//! this module just holds the selected provider + currency so handlers can call it
//! without knowing which provider is configured.

use std::sync::Arc;

use apex_edge_adapters_fiscal::{FiscalProvider, NoOpFiscalProvider};

#[derive(Clone)]
pub struct FiscalSettings {
    pub provider: Arc<dyn FiscalProvider + Send + Sync>,
    pub currency: String,
}

impl std::fmt::Debug for FiscalSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FiscalSettings")
            .field("provider", &self.provider.provider_code())
            .field("currency", &self.currency)
            .finish()
    }
}

impl Default for FiscalSettings {
    fn default() -> Self {
        Self {
            provider: Arc::new(NoOpFiscalProvider),
            currency: "USD".into(),
        }
    }
}
