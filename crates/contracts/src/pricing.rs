//! Price books and tax rules (HQ -> ApexEdge sync).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceBook {
    pub id: Uuid,
    pub name: String,
    pub effective_from: DateTime<Utc>,
    pub effective_until: Option<DateTime<Utc>>,
    pub entries: Vec<PriceBookEntry>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceBookEntry {
    pub item_id: Uuid,
    pub modifier_option_id: Option<Uuid>,
    pub price_cents: u64,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxRule {
    pub id: Uuid,
    pub tax_category_id: Uuid,
    pub rate_bps: u32,
    pub name: String,
    pub inclusive: bool,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaxQuoteLine {
    pub line_id: Uuid,
    pub tax_category_id: Uuid,
    pub taxable_amount_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaxBreakdown {
    pub line_id: Uuid,
    pub jurisdiction: String,
    pub rate_bps: u32,
    pub amount_cents: u64,
    pub inclusive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaxQuote {
    pub provider: String,
    pub currency: String,
    pub total_tax_cents: u64,
    pub breakdown: Vec<TaxBreakdown>,
}
