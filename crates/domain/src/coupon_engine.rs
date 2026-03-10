//! Coupon engine: eligibility, redemption limits, single-use (anti-replay).

use apex_edge_contracts::CouponDefinition;
use chrono::Utc;
use uuid::Uuid;

/// Coupon eligibility result.
#[derive(Debug, Clone)]
pub struct CouponEligibility {
    pub valid: bool,
    pub coupon_id: Uuid,
    pub code: String,
    pub discount_cents: u64,
    pub reason: Option<String>,
    /// Basket net after promo discounts (for capping coupon discount).
    pub basket_net_cents: u64,
}

/// Check coupon: valid window, not over redeemed. Caller provides redemption count for this coupon.
pub fn check_eligibility(
    def: &CouponDefinition,
    redemption_count_total: u64,
    redemption_count_customer: Option<u32>,
    basket_subtotal_cents: u64,
    promo_discount_cents: u64,
) -> CouponEligibility {
    let basket_net_cents = basket_subtotal_cents.saturating_sub(promo_discount_cents);
    let now = Utc::now();
    if now < def.valid_from {
        return CouponEligibility {
            valid: false,
            coupon_id: def.id,
            code: def.code.clone(),
            discount_cents: 0,
            reason: Some("coupon not yet valid".into()),
            basket_net_cents,
        };
    }
    if let Some(until) = def.valid_until {
        if now > until {
            return CouponEligibility {
                valid: false,
                coupon_id: def.id,
                code: def.code.clone(),
                discount_cents: 0,
                reason: Some("coupon expired".into()),
                basket_net_cents,
            };
        }
    }
    if let Some(max) = def.max_redemptions_total {
        if redemption_count_total >= max {
            return CouponEligibility {
                valid: false,
                coupon_id: def.id,
                code: def.code.clone(),
                discount_cents: 0,
                reason: Some("coupon redemption limit reached".into()),
                basket_net_cents,
            };
        }
    }
    if let Some(max) = def.max_redemptions_per_customer {
        if let Some(count) = redemption_count_customer {
            if count >= max {
                return CouponEligibility {
                    valid: false,
                    coupon_id: def.id,
                    code: def.code.clone(),
                    discount_cents: 0,
                    reason: Some("per-customer limit reached".into()),
                    basket_net_cents,
                };
            }
        }
    }
    CouponEligibility {
        valid: true,
        coupon_id: def.id,
        code: def.code.clone(),
        discount_cents: 0,
        reason: None,
        basket_net_cents,
    }
}

/// Compute coupon discount (amount comes from linked promo; we cap at basket net).
pub fn coupon_discount_cents(
    promo_discount_amount_cents: u64,
    basket_net_after_promos_cents: u64,
) -> u64 {
    promo_discount_amount_cents.min(basket_net_after_promos_cents)
}
