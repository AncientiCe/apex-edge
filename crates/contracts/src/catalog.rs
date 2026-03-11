//! Catalog contract: items, variants, options, inventory levels (HQ -> ApexEdge sync).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogItem {
    pub id: Uuid,
    pub sku: String,
    pub name: String,
    pub description: Option<String>,
    pub category_id: Uuid,
    pub tax_category_id: Uuid,
    pub modifiers: Vec<ModifierGroupRef>,
    pub is_active: bool,
    pub version: u64,
}

/// Per-item inventory level synced from HQ.
///
/// `available_qty` is the number of units available to sell at this store.
/// `is_available` reflects whether HQ considers this item sellable right now (may differ
/// from qty > 0 for backorder scenarios or manual overrides).
/// `image_urls` is an ordered list of product image URLs for the PDP gallery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryLevel {
    pub item_id: Uuid,
    pub available_qty: i64,
    pub is_available: bool,
    pub image_urls: Vec<String>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifierGroupRef {
    pub modifier_group_id: Uuid,
    pub min_selections: u32,
    pub max_selections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifierGroup {
    pub id: Uuid,
    pub name: String,
    pub options: Vec<ModifierOption>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifierOption {
    pub id: Uuid,
    pub name: String,
    pub price_offset_cents: i64,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub sort_order: u32,
    pub version: u64,
}
