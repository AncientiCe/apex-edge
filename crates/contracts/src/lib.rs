//! Canonical contracts for POS <-> ApexEdge and ApexEdge <-> HQ.
//! Schema versioning and compatibility: additive changes only; semantic version tags.

pub mod auth;
pub mod catalog;
pub mod config;
pub mod coupons;
pub mod customers;
pub mod hq;
pub mod pos;
pub mod pricing;
pub mod printing;
pub mod promotions;
pub mod version;

pub use auth::*;
pub use catalog::*;
pub use config::*;
pub use coupons::*;
pub use customers::*;
pub use hq::*;
pub use pos::*;
pub use pricing::*;
pub use printing::*;
pub use promotions::*;
pub use version::*;
