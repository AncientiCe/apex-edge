//! Coupons contract (HQ -> ApexEdge sync + redemption state).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouponDefinition {
    pub id: Uuid,
    pub code: String,
    pub promo_id: Uuid,
    pub max_redemptions_total: Option<u64>,
    pub max_redemptions_per_customer: Option<u32>,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouponRedemption {
    pub coupon_id: Uuid,
    pub order_id: Uuid,
    pub redeemed_at: DateTime<Utc>,
}
