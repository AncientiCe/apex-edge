//! Northbound API: POS <-> ApexEdge (HTTP).

pub mod auth;
pub mod catalog_categories;
pub mod catalog_search;
pub mod customer_search;
pub mod documents;
pub mod health;
pub mod metrics_handler;
pub mod pos;
pub mod pos_handler;
pub mod sync_status;

pub use auth::*;
pub use catalog_categories::*;
pub use catalog_search::*;
pub use customer_search::*;
pub use documents::*;
pub use health::*;
pub use metrics_handler::serve_metrics;
pub use pos::{get_cart_state_handler, handle_pos_command, AppState};
pub use sync_status::*;
