#![recursion_limit = "256"]

//! Northbound API: POS <-> ApexEdge (HTTP).

pub mod admin_api;
pub mod approvals;
pub mod audit;
pub mod auth;
pub mod catalog_categories;
pub mod catalog_search;
pub mod customer_search;
pub mod documents;
pub mod fiscal;
pub mod hardware;
pub mod health;
pub mod inventory_realtime;
pub mod metrics_handler;
pub mod openapi;
pub mod orders;
pub mod outbox_admin;
pub mod payments;
pub mod pos;
pub mod pos_handler;
pub mod rate_limit;
pub mod receipt_layout;
pub mod returns_handler;
pub mod role;
pub mod shifts_handler;
pub mod stream;
pub mod sync_status;

pub use admin_api::*;
pub use approvals::*;
pub use audit::*;
pub use auth::*;
pub use catalog_categories::*;
pub use catalog_search::*;
pub use customer_search::*;
pub use documents::*;
pub use fiscal::*;
pub use hardware::{DrawerKickPolicy, HardwareSettings};
pub use health::*;
pub use inventory_realtime::*;
pub use metrics_handler::serve_metrics;
pub use openapi::*;
pub use orders::*;
pub use outbox_admin::*;
pub use payments::*;
pub use pos::{get_cart_state_handler, handle_pos_command, AppState};
pub use rate_limit::{rate_limit_middleware, RateLimitSettings, RateLimiter};
pub use role::*;
pub use stream::{
    list_registers, pos_snapshot, pos_stream_sse, pos_stream_ws, stream_broadcast, StreamHub,
    StreamKind,
};
pub use sync_status::*;
