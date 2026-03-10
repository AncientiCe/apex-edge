//! Catalog contract: items, variants, options (HQ -> ApexEdge sync).

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
