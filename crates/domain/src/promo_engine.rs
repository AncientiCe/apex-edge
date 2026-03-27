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
    apply_promos_with_attribution(lines, category_by_item, promos, subtotal_cents).0
}

/// Compute line discounts and the set of promotions that contributed non-zero discount.
pub fn apply_promos_with_attribution(
    lines: &[CartLineItem],
    category_by_item: impl Fn(Uuid) -> Uuid,
    promos: &[Promotion],
    subtotal_cents: u64,
) -> (Vec<LinePriceResult>, Vec<Uuid>) {
    let mut line_discounts: std::collections::HashMap<Uuid, u64> =
        lines.iter().map(|l| (l.line_id, 0u64)).collect();
    let mut line_totals: std::collections::HashMap<Uuid, u64> = lines
        .iter()
        .map(|l| (l.line_id, l.line_total_cents))
        .collect();
    let mut applied_promo_ids: Vec<Uuid> = Vec::new();

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
            &promo.conditions,
            &category_by_item,
        );
        if discount > 0 {
            applied_promo_ids.push(promo.id);
        }
        allocate_discount_to_lines(
            &promo.actions,
            lines,
            &mut line_discounts,
            &mut line_totals,
            discount,
            &promo.conditions,
            &category_by_item,
        );
    }

    let line_results = lines
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
        .collect();
    (line_results, applied_promo_ids)
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

fn effective_max_units(
    lines: &[CartLineItem],
    actions: &[PromoAction],
    conditions: &[PromoCondition],
    category_by_item: &impl Fn(Uuid) -> Uuid,
) -> Option<u32> {
    let target_action = actions.iter().find_map(|action| match action {
        PromoAction::ApplyToItem {
            item_id,
            max_quantity,
        } => Some((Some(*item_id), None, *max_quantity)),
        PromoAction::ApplyToCategory {
            category_id,
            max_quantity,
        } => Some((None, Some(*category_id), *max_quantity)),
        PromoAction::ApplyToBasket => None,
    })?;
    let (item_target, category_target, action_max_quantity) = target_action;
    let condition_min_quantity = conditions.iter().find_map(|condition| match condition {
        PromoCondition::ItemInBasket {
            item_id,
            min_quantity,
        } if item_target == Some(*item_id) => Some(*min_quantity),
        PromoCondition::CategoryInBasket {
            category_id,
            min_quantity,
        } if category_target == Some(*category_id) => Some(*min_quantity),
        _ => None,
    });
    let base_max_units = action_max_quantity.or(condition_min_quantity)?;
    let repeat_groups = if let Some(trigger_units) = condition_min_quantity {
        if trigger_units == 0 {
            1
        } else {
            let matching_units: u32 = lines
                .iter()
                .filter(|line| {
                    item_target
                        .map(|item_id| line.item_id == item_id)
                        .or_else(|| {
                            category_target
                                .map(|category_id| category_by_item(line.item_id) == category_id)
                        })
                        .unwrap_or(false)
                })
                .map(|line| line.quantity)
                .sum();
            (matching_units / trigger_units).max(1)
        }
    } else {
        1
    };
    Some(base_max_units.saturating_mul(repeat_groups))
}

/// (line, eligible_quantity) where eligible_quantity is capped by promo max_quantity so only the first N units get the discount.
fn applicable_lines_with_cap<'a>(
    lines: &'a [CartLineItem],
    line_totals: &std::collections::HashMap<Uuid, u64>,
    actions: &[PromoAction],
    max_units: Option<u32>,
    category_by_item: &impl Fn(Uuid) -> Uuid,
) -> Vec<(&'a CartLineItem, u32)> {
    let mut applicable: Vec<_> = lines
        .iter()
        .filter(|l| action_applies_to_line(l, actions, category_by_item))
        .collect();
    applicable.sort_by_key(|l| l.line_id);
    let mut remaining = max_units.unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(applicable.len());
    for line in applicable {
        let line_total = line_totals.get(&line.line_id).unwrap_or(&0);
        if *line_total == 0 {
            continue;
        }
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
    conditions: &[PromoCondition],
    category_by_item: &impl Fn(Uuid) -> Uuid,
) -> u64 {
    let max_units = effective_max_units(lines, actions, conditions, category_by_item);
    let capped =
        applicable_lines_with_cap(lines, line_totals, actions, max_units, category_by_item);
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
        PromotionType::BuyXGetY {
            buy_quantity,
            get_quantity,
        } => {
            let bundle_size = buy_quantity.saturating_add(*get_quantity);
            if bundle_size == 0 || *get_quantity == 0 {
                return 0;
            }
            let mut eligible_units: Vec<(u64, u32)> = capped
                .iter()
                .filter_map(|(line, eligible_qty)| {
                    if *eligible_qty == 0 || line.quantity == 0 {
                        return None;
                    }
                    let line_total = *line_totals.get(&line.line_id).unwrap_or(&0);
                    if line_total == 0 {
                        return None;
                    }
                    let eligible_cents =
                        line_total.saturating_mul(*eligible_qty as u64) / (line.quantity as u64);
                    let unit_price = eligible_cents / (*eligible_qty as u64);
                    Some((unit_price, *eligible_qty))
                })
                .collect();
            let total_units: u32 = eligible_units.iter().map(|(_, qty)| *qty).sum();
            if total_units < bundle_size {
                return 0;
            }
            let mut free_units = (total_units / bundle_size).saturating_mul(*get_quantity);
            if free_units == 0 {
                return 0;
            }
            eligible_units.sort_by_key(|(unit_price, _)| *unit_price);
            let mut discount = 0u64;
            for (unit_price, qty) in eligible_units {
                if free_units == 0 {
                    break;
                }
                let take = qty.min(free_units);
                discount = discount.saturating_add(unit_price.saturating_mul(take as u64));
                free_units = free_units.saturating_sub(take);
            }
            discount.min(applicable_total)
        }
        PromotionType::PriceOverride { price_cents } => capped
            .iter()
            .map(|(line, eligible_qty)| {
                if *eligible_qty == 0 || line.quantity == 0 {
                    return 0u64;
                }
                let line_total = *line_totals.get(&line.line_id).unwrap_or(&0);
                if line_total == 0 {
                    return 0u64;
                }
                let eligible_cents =
                    line_total.saturating_mul(*eligible_qty as u64) / (line.quantity as u64);
                let current_unit = eligible_cents / (*eligible_qty as u64);
                if current_unit <= *price_cents {
                    return 0u64;
                }
                current_unit
                    .saturating_sub(*price_cents)
                    .saturating_mul(*eligible_qty as u64)
            })
            .sum::<u64>()
            .min(applicable_total),
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
    conditions: &[PromoCondition],
    category_by_item: &impl Fn(Uuid) -> Uuid,
) {
    let max_units = effective_max_units(lines, actions, conditions, category_by_item);
    let capped =
        applicable_lines_with_cap(lines, line_totals, actions, max_units, category_by_item);
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

    fn line_with_qty(item_id: Uuid, unit_price_cents: u64, quantity: u32) -> CartLineItem {
        CartLineItem {
            line_id: Uuid::new_v4(),
            item_id,
            sku: "sku".into(),
            name: "name".into(),
            quantity,
            modifier_option_ids: vec![],
            notes: None,
            unit_price_cents,
            line_total_cents: unit_price_cents.saturating_mul(quantity as u64),
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
            code: None,
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
            code: None,
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
        let zero_count = priced.iter().filter(|l| l.discount_cents == 0).count();
        assert_eq!(zero_count, 1, "exactly one line should have no discount");
    }

    #[test]
    fn item_min_quantity_caps_discount_when_action_has_no_max_quantity() {
        let item = Uuid::new_v4();
        let lines = vec![line(item, 100), line(item, 100), line(item, 100)];
        let promo_without_action_cap = Promotion {
            id: Uuid::new_v4(),
            code: Some("BUY2_20".into()),
            name: "Buy 2 get 20% off eligible units".into(),
            promo_type: PromotionType::PercentageOff { percent_bps: 2000 },
            priority: 100,
            valid_from: Utc::now() - Duration::minutes(1),
            valid_until: Some(Utc::now() + Duration::minutes(1)),
            conditions: vec![PromoCondition::ItemInBasket {
                item_id: item,
                min_quantity: 2,
            }],
            actions: vec![PromoAction::ApplyToItem {
                item_id: item,
                max_quantity: None,
            }],
            version: 1,
        };

        let priced =
            apply_promos_to_lines(&lines, |_| Uuid::nil(), &[promo_without_action_cap], 300);
        let total_discount: u64 = priced.iter().map(|l| l.discount_cents).sum();
        assert_eq!(
            total_discount, 40,
            "only 2 units should receive 20% discount"
        );
    }

    #[test]
    fn item_min_quantity_cap_applies_for_single_line_with_quantity_three() {
        let item = Uuid::new_v4();
        let lines = vec![line_with_qty(item, 100, 3)];
        let promo_without_action_cap = Promotion {
            id: Uuid::new_v4(),
            code: Some("BUY2_20".into()),
            name: "Buy 2 get 20% off eligible units".into(),
            promo_type: PromotionType::PercentageOff { percent_bps: 2000 },
            priority: 100,
            valid_from: Utc::now() - Duration::minutes(1),
            valid_until: Some(Utc::now() + Duration::minutes(1)),
            conditions: vec![PromoCondition::ItemInBasket {
                item_id: item,
                min_quantity: 2,
            }],
            actions: vec![PromoAction::ApplyToItem {
                item_id: item,
                max_quantity: None,
            }],
            version: 1,
        };

        let priced =
            apply_promos_to_lines(&lines, |_| Uuid::nil(), &[promo_without_action_cap], 300);
        assert_eq!(
            priced[0].discount_cents, 40,
            "discount should apply to only 2 of 3 units"
        );
    }

    #[test]
    fn buy_x_get_y_discount_is_applied_for_eligible_units() {
        let item = Uuid::new_v4();
        let lines = vec![line_with_qty(item, 100, 3)];
        let promo = Promotion {
            id: Uuid::new_v4(),
            code: None,
            name: "Buy 2 get 1".into(),
            promo_type: PromotionType::BuyXGetY {
                buy_quantity: 2,
                get_quantity: 1,
            },
            priority: 100,
            valid_from: Utc::now() - Duration::minutes(1),
            valid_until: Some(Utc::now() + Duration::minutes(1)),
            conditions: vec![PromoCondition::ItemInBasket {
                item_id: item,
                min_quantity: 3,
            }],
            actions: vec![PromoAction::ApplyToItem {
                item_id: item,
                max_quantity: None,
            }],
            version: 1,
        };

        let priced = apply_promos_to_lines(&lines, |_| Uuid::nil(), &[promo], 300);
        assert_eq!(
            priced[0].discount_cents, 100,
            "one free unit expected for buy-2-get-1"
        );
    }

    #[test]
    fn price_override_reduces_price_for_eligible_units() {
        let item = Uuid::new_v4();
        let lines = vec![line_with_qty(item, 100, 2)];
        let promo = Promotion {
            id: Uuid::new_v4(),
            code: None,
            name: "Override to 60".into(),
            promo_type: PromotionType::PriceOverride { price_cents: 60 },
            priority: 100,
            valid_from: Utc::now() - Duration::minutes(1),
            valid_until: Some(Utc::now() + Duration::minutes(1)),
            conditions: vec![PromoCondition::ItemInBasket {
                item_id: item,
                min_quantity: 1,
            }],
            actions: vec![PromoAction::ApplyToItem {
                item_id: item,
                max_quantity: None,
            }],
            version: 1,
        };

        let priced = apply_promos_to_lines(&lines, |_| Uuid::nil(), &[promo], 200);
        assert_eq!(
            priced[0].discount_cents, 80,
            "override should discount each unit by 40"
        );
    }
}
