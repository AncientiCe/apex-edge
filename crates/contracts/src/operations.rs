//! In-store operations contracts: parked carts, register layouts, and time clock.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParkedCartSummary {
    pub parked_cart_id: Uuid,
    pub cart_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub note: Option<String>,
    pub total_cents: u64,
    pub line_count: usize,
    pub parked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterLayout {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Option<Uuid>,
    pub language: String,
    pub tiles: Vec<RegisterLayoutTile>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterLayoutTile {
    pub label: String,
    pub item_id: Option<Uuid>,
    pub sku: Option<String>,
    pub sort_order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimeClockEntry {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub associate_id: String,
    pub clocked_in_at: DateTime<Utc>,
    pub clocked_out_at: Option<DateTime<Utc>>,
}
