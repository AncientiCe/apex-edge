//! Promotion engine: apply promos by priority, compute discount per line (deterministic).

use apex_edge_contracts::{PromoAction, PromoCondition, Promotion, PromotionType};
use chrono::Utc;
use uuid::Uuid;

use crate::cart::CartLineItem;
use crate::pricing::LinePriceResult;

/// Compute discount per line from applied promos (priority order). Returns updated line results.
pub fn apply_promos_to_lines(
    lines: &[CartLineItem],
    category_by_item: impl Fn(Uuid) -> Uuid,
    promos: &[Promotion],
    subtotal_cents: u64,
) -> Vec<LinePriceResult> {
    let mut line_discounts: std::collections::HashMap<Uuid, u64> =
        lines.iter().map(|l| (l.line_id, 0u64)).collect();
    let mut line_totals: std::collections::HashMap<Uuid, u64> = lines
        .iter()
        .map(|l| (l.line_id, l.line_total_cents))
        .collect();

    let mut sorted_promos: Vec<_> = promos.iter().collect();
    sorted_promos.sort_by_key(|p| std::cmp::Reverse(p.priority));

    for promo in sorted_promos {
        let now = Utc::now();
        if now < promo.valid_from {
            continue;
        }
        if let Some(until) = promo.valid_until {
            if now > until {
                continue;
            }
        }
        if !conditions_met(lines, subtotal_cents, &promo.conditions, &category_by_item) {
            continue;
        }
        let discount = compute_promo_discount(
            &promo.promo_type,
            lines,
            &line_totals,
            &promo.actions,
            &category_by_item,
        );
        allocate_discount_to_lines(
            &promo.actions,
            lines,
            &mut line_discounts,
            &mut line_totals,
            discount,
            &category_by_item,
        );
    }

    lines
        .iter()
        .map(|l| {
            let discount = *line_discounts.get(&l.line_id).unwrap_or(&0);
            let _line_net = l.line_total_cents.saturating_sub(discount);
            LinePriceResult {
                line_id: l.line_id,
                unit_price_cents: l.unit_price_cents,
                line_total_cents: l.line_total_cents,
                discount_cents: discount,
                tax_cents: l.tax_cents,
            }
        })
        .collect()
}

fn conditions_met(
    lines: &[CartLineItem],
    subtotal_cents: u64,
    conditions: &[PromoCondition],
    category_by_item: impl Fn(Uuid) -> Uuid,
) -> bool {
    for c in conditions {
        match c {
            PromoCondition::MinBasketAmount { amount_cents } => {
                if subtotal_cents < *amount_cents {
                    return false;
                }
            }
            PromoCondition::ItemInBasket {
                item_id,
                min_quantity,
            } => {
                let q: u32 = lines
                    .iter()
                    .filter(|l| l.item_id == *item_id)
                    .map(|l| l.quantity)
                    .sum();
                if q < *min_quantity {
                    return false;
                }
            }
            PromoCondition::CategoryInBasket {
                category_id,
                min_quantity,
            } => {
                let q: u32 = lines
                    .iter()
                    .filter(|l| category_by_item(l.item_id) == *category_id)
                    .map(|l| l.quantity)
                    .sum();
                if q < *min_quantity {
                    return false;
                }
            }
        }
    }
    true
}

fn compute_promo_discount(
    promo_type: &PromotionType,
    lines: &[CartLineItem],
    line_totals: &std::collections::HashMap<Uuid, u64>,
    actions: &[PromoAction],
    category_by_item: &impl Fn(Uuid) -> Uuid,
) -> u64 {
    let applicable_total: u64 = lines
        .iter()
        .filter(|l| action_applies_to_line(l, actions, category_by_item))
        .map(|l| *line_totals.get(&l.line_id).unwrap_or(&0))
        .sum();
    if applicable_total == 0 {
        return 0;
    }
    match promo_type {
        PromotionType::PercentageOff { percent_bps } => {
            (applicable_total * (*percent_bps as u64)) / 10000
        }
        PromotionType::FixedAmountOff { amount_cents } => (*amount_cents).min(applicable_total),
        PromotionType::BuyXGetY { .. } => 0,
        PromotionType::PriceOverride { price_cents: _ } => 0,
    }
}

fn action_applies_to_line(
    line: &CartLineItem,
    actions: &[PromoAction],
    category_by_item: &impl Fn(Uuid) -> Uuid,
) -> bool {
    if actions.is_empty() {
        return true;
    }
    for a in actions {
        match a {
            PromoAction::ApplyToItem { item_id } => {
                if *item_id == line.item_id {
                    return true;
                }
            }
            PromoAction::ApplyToCategory { category_id } => {
                if *category_id == category_by_item(line.item_id) {
                    return true;
                }
            }
            PromoAction::ApplyToBasket => return true,
        }
    }
    false
}

fn allocate_discount_to_lines(
    actions: &[PromoAction],
    lines: &[CartLineItem],
    line_discounts: &mut std::collections::HashMap<Uuid, u64>,
    line_totals: &mut std::collections::HashMap<Uuid, u64>,
    total_discount: u64,
    category_by_item: &impl Fn(Uuid) -> Uuid,
) {
    let applicable: Vec<_> = lines
        .iter()
        .filter(|l| action_applies_to_line(l, actions, category_by_item))
        .collect();
    let sum: u64 = applicable
        .iter()
        .map(|l| *line_totals.get(&l.line_id).unwrap_or(&0))
        .sum();
    if sum == 0 {
        return;
    }
    let mut remaining = total_discount;
    for (i, line) in applicable.iter().enumerate() {
        let line_net = *line_totals.get(&line.line_id).unwrap_or(&0);
        let discount = if i == applicable.len() - 1 {
            remaining
        } else {
            (total_discount * line_net / sum)
                .min(remaining)
                .min(line_net)
        };
        remaining = remaining.saturating_sub(discount);
        *line_discounts.get_mut(&line.line_id).unwrap() += discount;
        *line_totals.get_mut(&line.line_id).unwrap() = line_net.saturating_sub(discount);
    }
}
