//! Catalog contract: items, variants, options, inventory levels (HQ -> ApexEdge sync).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExternalIdentifiers {
    pub sku: Option<String>,
    pub gtin: Option<String>,
    pub upc: Option<String>,
    pub ean13: Option<String>,
    pub jan: Option<String>,
    pub isbn: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProductImage {
    pub url: String,
    pub title: Option<String>,
    pub identifier: Option<String>,
    pub is_main: Option<bool>,
    pub alt_text: Option<String>,
    pub dominant_color: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub aspect_ratio: Option<f64>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtendedAttribute {
    pub name: String,
    pub value: String,
}

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
    /// MPOS compatibility fields (optional in sync payloads).
    pub title: Option<String>,
    pub brand: Option<String>,
    pub caption: Option<String>,
    pub external_identifiers: Option<ExternalIdentifiers>,
    pub images: Option<Vec<ProductImage>>,
    pub is_preorder: Option<bool>,
    pub online_from: Option<DateTime<Utc>>,
    pub serialized_inventory: Option<bool>,
    pub extended_attributes: Option<Vec<ExtendedAttribute>>,
    pub variations: Option<serde_json::Value>,
    pub variation_attributes: Option<serde_json::Value>,
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
