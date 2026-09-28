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
    /// True when `tax_cents` is contained in the price (VAT-inclusive) rather than added to it.
    #[serde(default)]
    pub tax_inclusive: bool,
}

/// Lookup base price for item + modifiers (from local price book).
///
/// # Examples
///
/// ```
/// use apex_edge_contracts::PriceBookEntry;
/// use apex_edge_domain::base_price_cents;
/// use uuid::Uuid;
///
/// let item_id = Uuid::new_v4();
/// let mod_id = Uuid::new_v4();
/// let entries = vec![
///     PriceBookEntry {
///         item_id,
///         modifier_option_id: None,
///         price_cents: 500,
///         currency: "USD".into(),
///     },
///     PriceBookEntry {
///         item_id,
///         modifier_option_id: Some(mod_id),
///         price_cents: 50,
///         currency: "USD".into(),
///     },
/// ];
///
/// assert_eq!(base_price_cents(item_id, &[mod_id], 2, &entries), 1100);
/// ```
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
///
/// # Examples
///
/// ```
/// use apex_edge_domain::apply_tax;
///
/// assert_eq!(apply_tax(1000, 1000, false), 100);
/// assert_eq!(apply_tax(1100, 1000, true), 100);
/// ```
pub fn apply_tax(amount_cents: u64, rate_bps: u32, inclusive: bool) -> u64 {
    if inclusive {
        amount_cents - (amount_cents * 10000 / (10000 + rate_bps as u64))
    } else {
        (amount_cents * (rate_bps as u64)) / 10000
    }
}

/// Tax on one line, and whether it is contained in the line's price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineTax {
    pub tax_cents: u64,
    pub inclusive: bool,
}

/// Tax for a line given its tax category and the synced rules.
///
/// `line_amount_cents` is the line's price after discounts. For an inclusive rule it already
/// contains the tax, which is extracted; for an exclusive rule the tax is added on top.
///
/// # Examples
///
/// ```
/// use apex_edge_contracts::TaxRule;
/// use apex_edge_domain::tax_for_line;
/// use uuid::Uuid;
///
/// let vat = Uuid::new_v4();
/// let rules = vec![TaxRule {
///     id: Uuid::new_v4(),
///     tax_category_id: vat,
///     rate_bps: 1000,
///     name: "VAT".into(),
///     inclusive: true,
///     version: 1,
/// }];
/// let tax = tax_for_line(1100, vat, &rules);
/// assert_eq!((tax.tax_cents, tax.inclusive), (100, true));
/// ```
pub fn tax_for_line(line_amount_cents: u64, tax_category_id: Uuid, rules: &[TaxRule]) -> LineTax {
    match rules.iter().find(|r| r.tax_category_id == tax_category_id) {
        Some(rule) => LineTax {
            tax_cents: apply_tax(line_amount_cents, rule.rate_bps, rule.inclusive),
            inclusive: rule.inclusive,
        },
        None => LineTax::default(),
    }
}

pub fn currency_minor_units(currency: &str) -> u32 {
    match currency.to_ascii_uppercase().as_str() {
        "BHD" | "JOD" | "KWD" | "OMR" | "TND" => 3,
        "CLP" | "ISK" | "JPY" | "KRW" | "PYG" | "VND" => 0,
        _ => 2,
    }
}

pub fn round_major_units_to_minor(major_units_micros: i64, currency: &str) -> i64 {
    let scale = 10_i64.pow(currency_minor_units(currency));
    let scaled = major_units_micros.saturating_mul(scale);
    if scaled >= 0 {
        (scaled + 500_000) / 1_000_000
    } else {
        (scaled - 500_000) / 1_000_000
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_tax, base_price_cents, currency_minor_units, round_major_units_to_minor};
    use apex_edge_contracts::PriceBookEntry;
    use uuid::Uuid;

    #[test]
    fn base_price_includes_modifiers_and_quantity() {
        let item = Uuid::new_v4();
        let m1 = Uuid::new_v4();
        let entries = vec![
            PriceBookEntry {
                item_id: item,
                modifier_option_id: None,
                price_cents: 200,
                currency: "USD".into(),
            },
            PriceBookEntry {
                item_id: item,
                modifier_option_id: Some(m1),
                price_cents: 50,
                currency: "USD".into(),
            },
        ];
        assert_eq!(base_price_cents(item, &[m1], 3, &entries), 750);
    }

    #[test]
    fn apply_tax_handles_exclusive_and_inclusive() {
        assert_eq!(apply_tax(1000, 1000, false), 100);
        assert_eq!(apply_tax(1100, 1000, true), 100);
    }

    #[test]
    fn currency_rounding_uses_iso_minor_units() {
        assert_eq!(currency_minor_units("USD"), 2);
        assert_eq!(currency_minor_units("JPY"), 0);
        assert_eq!(currency_minor_units("KWD"), 3);

        assert_eq!(round_major_units_to_minor(12_345_000, "USD"), 1235);
        assert_eq!(round_major_units_to_minor(12_500_000, "JPY"), 13);
        assert_eq!(round_major_units_to_minor(12_345_600, "KWD"), 12346);
    }
}
