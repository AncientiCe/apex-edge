//! ApexEdge library: reusable app bootstrap for binary and tests.

pub mod app;
pub mod http_metrics_layer;

pub use app::build_router;
