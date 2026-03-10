//! Deterministic pricing pipeline: base price -> override -> promo -> coupon -> tax -> rounding.

use apex_edge_contracts::{PriceBookEntry, TaxRule};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Result for one cart line after full pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinePriceResult {
    pub line_id: Uuid,
    pub unit_price_cents: u64,
    pub line_total_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
}

/// Lookup base price for item + modifiers (from local price book).
pub fn base_price_cents(
    item_id: Uuid,
    modifier_option_ids: &[Uuid],
    quantity: u32,
    entries: &[PriceBookEntry],
) -> u64 {
    let mut cents: u64 = 0;
    if let Some(e) = entries
        .iter()
        .find(|e| e.item_id == item_id && e.modifier_option_id.is_none())
    {
        cents = e.price_cents;
    }
    for &mod_id in modifier_option_ids {
        if let Some(e) = entries
            .iter()
            .find(|e| e.item_id == item_id && e.modifier_option_id == Some(mod_id))
        {
            cents = cents.saturating_add(e.price_cents);
        }
    }
    cents.saturating_mul(quantity as u64)
}

/// Apply tax (inclusive or exclusive) to amount. rate_bps = basis points (e.g. 1000 = 10%).
pub fn apply_tax(amount_cents: u64, rate_bps: u32, inclusive: bool) -> u64 {
    if inclusive {
        amount_cents - (amount_cents * 10000 / (10000 + rate_bps as u64))
    } else {
        (amount_cents * (rate_bps as u64)) / 10000
    }
}

/// Get tax amount for a line given tax category and rules.
pub fn tax_for_line(
    line_net_cents: u64,
    tax_category_id: Uuid,
    rules: &[TaxRule],
    inclusive: bool,
) -> u64 {
    let rule = match rules.iter().find(|r| r.tax_category_id == tax_category_id) {
        Some(r) => r,
        None => return 0,
    };
    apply_tax(line_net_cents, rule.rate_bps, inclusive)
}
