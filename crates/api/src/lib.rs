//! Northbound API: POS <-> ApexEdge (HTTP).

pub mod approvals;
pub mod audit;
pub mod auth;
pub mod catalog_categories;
pub mod catalog_search;
pub mod customer_search;
pub mod documents;
pub mod health;
pub mod metrics_handler;
pub mod openapi;
pub mod pos;
pub mod pos_handler;
pub mod returns_handler;
pub mod role;
pub mod shifts_handler;
pub mod stream;
pub mod sync_status;

pub use approvals::*;
pub use audit::*;
pub use auth::*;
pub use catalog_categories::*;
pub use catalog_search::*;
pub use customer_search::*;
pub use documents::*;
pub use health::*;
pub use metrics_handler::serve_metrics;
pub use openapi::*;
pub use pos::{get_cart_state_handler, handle_pos_command, AppState};
pub use role::*;
pub use stream::{pos_stream_sse, pos_stream_ws, stream_broadcast, StreamHub, StreamKind};
pub use sync_status::*;
