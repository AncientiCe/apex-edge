//! Promotions contract (HQ -> ApexEdge sync).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Promotion {
    pub id: Uuid,
    pub code: Option<String>,
    pub name: String,
    pub promo_type: PromotionType,
    pub priority: u32,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub conditions: Vec<PromoCondition>,
    pub actions: Vec<PromoAction>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromotionType {
    PercentageOff {
        percent_bps: u32,
    },
    FixedAmountOff {
        amount_cents: u64,
    },
    BuyXGetY {
        buy_quantity: u32,
        get_quantity: u32,
    },
    PriceOverride {
        price_cents: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromoCondition {
    MinBasketAmount {
        amount_cents: u64,
    },
    ItemInBasket {
        item_id: Uuid,
        min_quantity: u32,
    },
    CategoryInBasket {
        category_id: Uuid,
        min_quantity: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromoAction {
    ApplyToItem { item_id: Uuid },
    ApplyToCategory { category_id: Uuid },
    ApplyToBasket,
}
