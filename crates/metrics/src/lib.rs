//! Shared Prometheus-style metric names, label keys, and cardinality contract.
//!
//! All metric names use snake_case and the `apex_edge_` prefix.
//! Labels are bounded: use only the keys and value sets defined here to avoid cardinality explosion.

pub mod schema;

pub use schema::*;

/// Re-export so the app can pass the handle to the router and expose `/metrics`.
pub use metrics_exporter_prometheus::PrometheusHandle;

use metrics_exporter_prometheus::PrometheusBuilder;

/// Installs the global Prometheus recorder and returns a handle to render scrape output.
/// Call once at startup (e.g. in `main`) before building the router; pass the handle
/// into the app so the `/metrics` route can call `handle.render()`.
pub fn install_recorder() -> Result<PrometheusHandle, Box<dyn std::error::Error + Send + Sync>> {
    PrometheusBuilder::new()
        .install_recorder()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
}
