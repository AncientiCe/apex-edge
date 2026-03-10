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

/// (line, eligible_quantity) where eligible_quantity is capped by promo max_quantity so only the first N units get the discount.
fn applicable_lines_with_cap<'a>(
    lines: &'a [CartLineItem],
    actions: &[PromoAction],
    category_by_item: &impl Fn(Uuid) -> Uuid,
) -> Vec<(&'a CartLineItem, u32)> {
    let mut applicable: Vec<_> = lines
        .iter()
        .filter(|l| action_applies_to_line(l, actions, category_by_item))
        .collect();
    applicable.sort_by_key(|l| l.line_id);
    let max_units = actions.iter().find_map(|a| match a {
        PromoAction::ApplyToItem { max_quantity, .. }
        | PromoAction::ApplyToCategory { max_quantity, .. } => *max_quantity,
        PromoAction::ApplyToBasket => None,
    });
    let mut remaining = max_units.unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(applicable.len());
    for line in applicable {
        let eligible = (line.quantity).min(remaining);
        remaining = remaining.saturating_sub(eligible);
        out.push((line, eligible));
        if remaining == 0 {
            break;
        }
    }
    out
}

fn compute_promo_discount(
    promo_type: &PromotionType,
    lines: &[CartLineItem],
    line_totals: &std::collections::HashMap<Uuid, u64>,
    actions: &[PromoAction],
    category_by_item: &impl Fn(Uuid) -> Uuid,
) -> u64 {
    let capped = applicable_lines_with_cap(lines, line_totals, actions, category_by_item);
    let applicable_total: u64 = capped
        .iter()
        .map(|(l, eligible_qty)| {
            let line_total = *line_totals.get(&l.line_id).unwrap_or(&0);
            if l.quantity == 0 {
                0u64
            } else {
                line_total * (*eligible_qty as u64) / (l.quantity as u64)
            }
        })
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
            PromoAction::ApplyToItem { item_id, .. } => {
                if *item_id == line.item_id {
                    return true;
                }
            }
            PromoAction::ApplyToCategory { category_id, .. } => {
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
    let capped = applicable_lines_with_cap(lines, line_totals, actions, category_by_item);
    let eligible_total_cents: u64 = capped
        .iter()
        .map(|(l, eligible_qty)| {
            let line_total = *line_totals.get(&l.line_id).unwrap_or(&0);
            if l.quantity == 0 {
                0u64
            } else {
                line_total * (*eligible_qty as u64) / (l.quantity as u64)
            }
        })
        .sum();
    if eligible_total_cents == 0 {
        return;
    }
    let mut remaining = total_discount;
    for (i, (line, eligible_qty)) in capped.iter().enumerate() {
        let line_total = *line_totals.get(&line.line_id).unwrap_or(&0);
        let line_eligible_cents = if line.quantity == 0 {
            0u64
        } else {
            line_total * (*eligible_qty as u64) / (line.quantity as u64)
        };
        let discount = if line_eligible_cents == 0 {
            0u64
        } else if i == capped.len() - 1 {
            remaining
        } else {
            (total_discount * line_eligible_cents / eligible_total_cents)
                .min(remaining)
                .min(line_eligible_cents)
        };
        remaining = remaining.saturating_sub(discount);
        *line_discounts.get_mut(&line.line_id).unwrap() += discount;
        *line_totals.get_mut(&line.line_id).unwrap() = line_total.saturating_sub(discount);
    }
}

#[cfg(test)]
mod tests {
    use super::apply_promos_to_lines;
    use crate::cart::CartLineItem;
    use apex_edge_contracts::{PromoAction, PromoCondition, Promotion, PromotionType};
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    fn line(item_id: Uuid, total: u64) -> CartLineItem {
        CartLineItem {
            line_id: Uuid::new_v4(),
            item_id,
            sku: "sku".into(),
            name: "name".into(),
            quantity: 1,
            modifier_option_ids: vec![],
            notes: None,
            unit_price_cents: total,
            line_total_cents: total,
            discount_cents: 0,
            tax_cents: 0,
        }
    }

    #[test]
    fn percentage_promo_allocates_discount_to_applicable_lines() {
        let item_a = Uuid::new_v4();
        let item_b = Uuid::new_v4();
        let lines = vec![line(item_a, 1000), line(item_b, 1000)];
        let promo = Promotion {
            id: Uuid::new_v4(),
            code: Some("P10".into()),
            name: "10 off".into(),
            promo_type: PromotionType::PercentageOff { percent_bps: 1000 },
            priority: 10,
            valid_from: Utc::now() - Duration::minutes(1),
            valid_until: Some(Utc::now() + Duration::minutes(1)),
            conditions: vec![],
            actions: vec![PromoAction::ApplyToBasket],
            version: 1,
        };
        let priced = apply_promos_to_lines(&lines, |_| Uuid::nil(), &[promo], 2000);
        let total_discount: u64 = priced.iter().map(|l| l.discount_cents).sum();
        assert_eq!(total_discount, 200);
    }

    #[test]
    fn promo_with_unmet_condition_does_not_apply() {
        let item = Uuid::new_v4();
        let lines = vec![line(item, 500)];
        let promo = Promotion {
            id: Uuid::new_v4(),
            code: None,
            name: "min basket".into(),
            promo_type: PromotionType::FixedAmountOff { amount_cents: 100 },
            priority: 1,
            valid_from: Utc::now() - Duration::minutes(1),
            valid_until: Some(Utc::now() + Duration::minutes(1)),
            conditions: vec![PromoCondition::MinBasketAmount { amount_cents: 9999 }],
            actions: vec![PromoAction::ApplyToBasket],
            version: 1,
        };
        let priced = apply_promos_to_lines(&lines, |_| Uuid::nil(), &[promo], 500);
        assert_eq!(priced[0].discount_cents, 0);
    }

    #[test]
    fn buy_2_get_50_percent_off_each_applies_only_to_two_units() {
        let item = Uuid::new_v4();
        let line1 = line(item, 310);
        let line2 = line(item, 310);
        let line3 = line(item, 310);
        let lines = vec![line1, line2, line3];
        let promo = Promotion {
            id: Uuid::new_v4(),
            code: Some("BUY2_50".into()),
            name: "Buy 2 get 50% off each".into(),
            promo_type: PromotionType::PercentageOff { percent_bps: 5000 },
            priority: 20,
            valid_from: Utc::now() - Duration::minutes(1),
            valid_until: Some(Utc::now() + Duration::minutes(1)),
            conditions: vec![PromoCondition::ItemInBasket {
                item_id: item,
                min_quantity: 2,
            }],
            actions: vec![PromoAction::ApplyToItem {
                item_id: item,
                max_quantity: Some(2),
            }],
            version: 1,
        };
        let priced = apply_promos_to_lines(&lines, |_| Uuid::nil(), &[promo], 930);
        let total_discount: u64 = priced.iter().map(|l| l.discount_cents).sum();
        let expected_discount = (310 + 310) * 5000 / 10000;
        assert_eq!(
            total_discount, expected_discount,
            "discount only on first 2 units"
        );
        assert_eq!(
            priced[0].discount_cents + priced[1].discount_cents,
            expected_discount
        );
        assert_eq!(priced[2].discount_cents, 0, "third item gets no discount");
    }
}
