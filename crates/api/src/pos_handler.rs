//! POS command execution: load/save cart, run pricing pipeline, return payloads.

use apex_edge_contracts::{
    build_submission_envelope, AppliedPromoInfo, CartState, ContractVersion, FinalizeResult,
    ManualDiscountInfo, ManualDiscountKind, PosCommand, PosError, PosRequestEnvelope,
    PosResponseEnvelope,
};
use apex_edge_domain::{apply_promos_with_attribution, base_price_cents, tax_for_line, Cart};
use apex_edge_printing::generate_document;
use apex_edge_storage::{
    get_catalog_item, get_customer, insert_outbox, list_price_book_entries, list_promotions,
    list_tax_rules, load_cart, save_cart,
};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pos::AppState;

fn cart_state_to_payload(state: &CartState) -> serde_json::Value {
    serde_json::to_value(state).unwrap_or(serde_json::Value::Null)
}

/// Build a `CartState` from a `Cart`.
pub async fn build_cart_state(pool: &SqlitePool, store_id: Uuid, cart: &Cart) -> CartState {
    tracing::debug!(store_id = %store_id, pool_size = std::mem::size_of_val(pool), "building cart state");
    let mut state = cart.to_cart_state();
    if !cart.applied_promo_ids.is_empty() {
        let promo_lookup = list_promotions(pool, store_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|promo| (promo.id, promo))
            .collect::<std::collections::HashMap<_, _>>();
        state.applied_promos = cart
            .applied_promo_ids
            .iter()
            .map(|promo_id| {
                if let Some(promo) = promo_lookup.get(promo_id) {
                    AppliedPromoInfo {
                        promo_id: *promo_id,
                        name: promo.name.clone(),
                        code: promo.code.clone(),
                    }
                } else {
                    AppliedPromoInfo {
                        promo_id: *promo_id,
                        name: promo_id.to_string(),
                        code: None,
                    }
                }
            })
            .collect();
    }
    state
}

fn finalize_result_to_payload(result: &FinalizeResult) -> serde_json::Value {
    serde_json::to_value(result).unwrap_or(serde_json::Value::Null)
}

pub async fn load_cart_from_db(
    pool: &SqlitePool,
    cart_id: Uuid,
) -> Result<Option<Cart>, Vec<PosError>> {
    let row = load_cart(pool, cart_id).await.map_err(|e| {
        vec![PosError {
            code: "LOAD_CART_FAILED".into(),
            message: e.to_string(),
            field: None,
        }]
    })?;
    let Some(row) = row else {
        return Ok(None);
    };
    let cart: Cart = serde_json::from_value(row.data.clone()).map_err(|e| {
        vec![PosError {
            code: "CART_DESERIALIZE".into(),
            message: e.to_string(),
            field: None,
        }]
    })?;
    Ok(Some(cart))
}

pub async fn save_cart_to_db(pool: &SqlitePool, cart: &Cart) -> Result<(), Vec<PosError>> {
    let data = serde_json::to_value(cart).map_err(|e| {
        vec![PosError {
            code: "CART_SERIALIZE".into(),
            message: e.to_string(),
            field: None,
        }]
    })?;
    save_cart(
        pool,
        cart.id,
        cart.store_id,
        cart.register_id,
        &cart.state,
        &data,
    )
    .await
    .map_err(|e| {
        vec![PosError {
            code: "SAVE_CART_FAILED".into(),
            message: e.to_string(),
            field: None,
        }]
    })?;
    Ok(())
}

/// Run pricing pipeline (promos + tax) and apply results to cart.
pub async fn run_pricing_pipeline(
    pool: &SqlitePool,
    store_id: Uuid,
    cart: &mut Cart,
) -> Result<(), Vec<PosError>> {
    let rules = list_tax_rules(pool, store_id).await.map_err(|e| {
        vec![PosError {
            code: "TAX_RULES".into(),
            message: e.to_string(),
            field: None,
        }]
    })?;
    let promos = list_promotions(pool, store_id).await.map_err(|e| {
        vec![PosError {
            code: "PROMOTIONS".into(),
            message: e.to_string(),
            field: None,
        }]
    })?;

    let mut item_to_tax_category: std::collections::HashMap<Uuid, Uuid> =
        std::collections::HashMap::new();
    for line in &cart.lines {
        if let Ok(Some(item)) = get_catalog_item(pool, store_id, line.item_id).await {
            item_to_tax_category.insert(line.item_id, item.tax_category_id);
        }
    }

    let category_by_item =
        |item_id: Uuid| *item_to_tax_category.get(&item_id).unwrap_or(&Uuid::nil());
    let subtotal = cart.subtotal_cents();
    let (mut results, applied_promo_ids) =
        apply_promos_with_attribution(&cart.lines, category_by_item, &promos, subtotal);

    for res in &mut results {
        let line = cart
            .lines
            .iter()
            .find(|l| l.line_id == res.line_id)
            .unwrap();
        let tax_cat = category_by_item(line.item_id);
        let line_net = res.line_total_cents.saturating_sub(res.discount_cents);
        res.tax_cents = tax_for_line(line_net, tax_cat, &rules, false);
    }

    let mut applied_any = false;
    for res in &results {
        if res.discount_cents > 0 {
            applied_any = true;
            break;
        }
    }
    cart.applied_promo_ids = applied_promo_ids;
    cart.apply_pricing(results);

    // Apply manual discounts (stored with reason); add to line discount_cents and recalc tax.
    apply_manual_discounts_to_lines(cart, &rules, &item_to_tax_category)?;
    if applied_any || !cart.manual_discounts.is_empty() {
        cart.set_discounted();
    }
    Ok(())
}

/// Apply stored manual discounts to lines (add amount to line.discount_cents) and recalc tax.
fn apply_manual_discounts_to_lines(
    cart: &mut Cart,
    rules: &[apex_edge_contracts::TaxRule],
    item_to_tax_category: &std::collections::HashMap<Uuid, Uuid>,
) -> Result<(), Vec<PosError>> {
    if cart.manual_discounts.is_empty() {
        return Ok(());
    }
    let category_by_item =
        |item_id: Uuid| *item_to_tax_category.get(&item_id).unwrap_or(&Uuid::nil());

    for md in &cart.manual_discounts.clone() {
        let amount = md.amount_cents;
        if amount == 0 {
            continue;
        }
        if let Some(line_id) = md.line_id {
            if let Some(line) = cart.lines.iter_mut().find(|l| l.line_id == line_id) {
                let line_net = line.line_total_cents.saturating_sub(line.discount_cents);
                let add = amount.min(line_net);
                line.discount_cents = line.discount_cents.saturating_add(add);
            }
        } else {
            let total_net: u64 = cart
                .lines
                .iter()
                .map(|l| l.line_total_cents.saturating_sub(l.discount_cents))
                .sum();
            if total_net == 0 {
                continue;
            }
            let mut remaining = amount;
            let line_count = cart.lines.len();
            for (i, line) in cart.lines.iter_mut().enumerate() {
                let line_net = line.line_total_cents.saturating_sub(line.discount_cents);
                let add = if i == line_count - 1 {
                    remaining.min(line_net)
                } else {
                    (amount * line_net / total_net).min(remaining).min(line_net)
                };
                remaining = remaining.saturating_sub(add);
                line.discount_cents = line.discount_cents.saturating_add(add);
            }
        }
    }

    for line in &mut cart.lines {
        let tax_cat = category_by_item(line.item_id);
        let line_net = line.line_total_cents.saturating_sub(line.discount_cents);
        line.tax_cents = tax_for_line(line_net, tax_cat, rules, false);
    }
    Ok(())
}

pub async fn execute_pos_command(
    app: &AppState,
    envelope: PosRequestEnvelope<PosCommand>,
) -> PosResponseEnvelope<serde_json::Value> {
    let idempotency_key = envelope.idempotency_key;
    let store_id = envelope.store_id;
    let register_id = envelope.register_id;
    let pool = &app.pool;

    let result = match &envelope.payload {
        PosCommand::CreateCart(p) => {
            let cart_id = p.cart_id.unwrap_or_else(Uuid::new_v4);
            let cart = Cart::new(cart_id, store_id, register_id);
            if let Err(errors) = save_cart_to_db(pool, &cart).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            let state = build_cart_state(pool, store_id, &cart).await;
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(cart_state_to_payload(&state)),
                errors: vec![],
            }
        }
        PosCommand::SetCustomer(p) => {
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CART_NOT_FOUND".into(),
                        message: "Cart not found".into(),
                        field: None,
                    }],
                };
            };
            if get_customer(pool, store_id, p.customer_id)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CUSTOMER_NOT_FOUND".into(),
                        message: "Customer not found".into(),
                        field: None,
                    }],
                };
            }
            cart.set_customer(p.customer_id);
            if let Err(errors) = save_cart_to_db(pool, &cart).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            let state = build_cart_state(pool, store_id, &cart).await;
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(cart_state_to_payload(&state)),
                errors: vec![],
            }
        }
        PosCommand::AddLineItem(p) => {
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CART_NOT_FOUND".into(),
                        message: "Cart not found".into(),
                        field: None,
                    }],
                };
            };
            if cart.ensure_can_edit().is_err() {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_STATE".into(),
                        message: "Cart cannot be edited".into(),
                        field: None,
                    }],
                };
            }
            let Some(item) = get_catalog_item(pool, store_id, p.item_id)
                .await
                .ok()
                .flatten()
            else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "ITEM_NOT_FOUND".into(),
                        message: "Catalog item not found".into(),
                        field: None,
                    }],
                };
            };
            if let Some(stock_error) = item.check_quantity(p.quantity as i64) {
                metrics::counter!(
                    apex_edge_metrics::CATALOG_STOCK_CHECKS_TOTAL,
                    1u64,
                    "outcome" => stock_error
                );
                let message = match stock_error {
                    "OUT_OF_STOCK" => "Item is out of stock".into(),
                    "INSUFFICIENT_STOCK" => format!(
                        "Requested quantity {} exceeds available stock ({})",
                        p.quantity,
                        item.available_qty.unwrap_or(0)
                    ),
                    other => format!("Stock check failed: {other}"),
                };
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: stock_error.into(),
                        message,
                        field: None,
                    }],
                };
            }
            metrics::counter!(
                apex_edge_metrics::CATALOG_STOCK_CHECKS_TOTAL,
                1u64,
                "outcome" => "ok"
            );
            let entries = list_price_book_entries(pool, store_id).await.map_err(|e| {
                vec![PosError {
                    code: "PRICE_BOOK".into(),
                    message: e.to_string(),
                    field: None,
                }]
            });
            let entries: Vec<_> = match entries {
                Ok(e) => e,
                Err(errors) => {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors,
                    };
                }
            };
            let base_cents =
                base_price_cents(p.item_id, &p.modifier_option_ids, p.quantity, &entries);
            let unit_price = if let Some(override_cents) = p.unit_price_override_cents {
                if override_cents > 0 {
                    override_cents
                } else if p.quantity > 0 {
                    base_cents / (p.quantity as u64)
                } else {
                    0
                }
            } else if p.quantity > 0 {
                base_cents / (p.quantity as u64)
            } else {
                0
            };
            let line_id = Uuid::new_v4();
            cart.add_line_item(apex_edge_domain::cart::AddLineItemInput {
                line_id,
                item_id: p.item_id,
                sku: item.sku.clone(),
                name: item.name.clone(),
                quantity: p.quantity,
                unit_price_cents: unit_price,
                modifier_option_ids: p.modifier_option_ids.clone(),
                notes: p.notes.clone(),
            });
            if let Err(errors) = run_pricing_pipeline(pool, store_id, &mut cart).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            if let Err(errors) = save_cart_to_db(pool, &cart).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            let state = build_cart_state(pool, store_id, &cart).await;
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(cart_state_to_payload(&state)),
                errors: vec![],
            }
        }
        PosCommand::ApplyManualDiscount(p) => {
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CART_NOT_FOUND".into(),
                        message: "Cart not found".into(),
                        field: None,
                    }],
                };
            };
            if cart.ensure_can_edit().is_err() {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_STATE".into(),
                        message: "Cart cannot be edited".into(),
                        field: None,
                    }],
                };
            }
            let reason = p.reason.trim();
            if reason.is_empty() {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "REASON_REQUIRED".into(),
                        message: "Manual discount requires a reason".into(),
                        field: Some("reason".into()),
                    }],
                };
            }
            let subtotal = cart.subtotal_cents();
            let amount_cents = match p.kind {
                ManualDiscountKind::PercentCart => {
                    (subtotal.saturating_mul(p.value) / 10000).min(subtotal)
                }
                ManualDiscountKind::FixedCart => p.value.min(subtotal),
                ManualDiscountKind::PercentItem => {
                    let line_id = p.line_id.ok_or_else(|| PosError {
                        code: "LINE_ID_REQUIRED".into(),
                        message: "Percent per item requires line_id".into(),
                        field: Some("line_id".into()),
                    });
                    let line_id = match line_id {
                        Ok(id) => id,
                        Err(e) => {
                            return PosResponseEnvelope {
                                version: ContractVersion::V1_0_0,
                                success: false,
                                idempotency_key,
                                payload: None,
                                errors: vec![e],
                            };
                        }
                    };
                    let line = match cart.lines.iter().find(|l| l.line_id == line_id) {
                        Some(l) => l,
                        None => {
                            return PosResponseEnvelope {
                                version: ContractVersion::V1_0_0,
                                success: false,
                                idempotency_key,
                                payload: None,
                                errors: vec![PosError {
                                    code: "LINE_NOT_FOUND".into(),
                                    message: "Line not found".into(),
                                    field: None,
                                }],
                            };
                        }
                    };
                    let line_total = line.line_total_cents.saturating_sub(line.discount_cents);
                    (line_total.saturating_mul(p.value) / 10000).min(line_total)
                }
                ManualDiscountKind::FixedItem => {
                    let line_id = p.line_id.ok_or_else(|| PosError {
                        code: "LINE_ID_REQUIRED".into(),
                        message: "Fixed per item requires line_id".into(),
                        field: Some("line_id".into()),
                    });
                    let line_id = match line_id {
                        Ok(id) => id,
                        Err(e) => {
                            return PosResponseEnvelope {
                                version: ContractVersion::V1_0_0,
                                success: false,
                                idempotency_key,
                                payload: None,
                                errors: vec![e],
                            };
                        }
                    };
                    let line = match cart.lines.iter().find(|l| l.line_id == line_id) {
                        Some(l) => l,
                        None => {
                            return PosResponseEnvelope {
                                version: ContractVersion::V1_0_0,
                                success: false,
                                idempotency_key,
                                payload: None,
                                errors: vec![PosError {
                                    code: "LINE_NOT_FOUND".into(),
                                    message: "Line not found".into(),
                                    field: None,
                                }],
                            };
                        }
                    };
                    let line_net = line.line_total_cents.saturating_sub(line.discount_cents);
                    p.value.min(line_net)
                }
            };
            if amount_cents == 0 {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "ZERO_DISCOUNT".into(),
                        message: "Computed discount amount is zero".into(),
                        field: None,
                    }],
                };
            }
            let line_id = match p.kind {
                ManualDiscountKind::PercentItem | ManualDiscountKind::FixedItem => p.line_id,
                _ => None,
            };
            cart.manual_discounts.push(ManualDiscountInfo {
                reason: reason.to_string(),
                amount_cents,
                line_id,
            });
            let rules = match list_tax_rules(pool, store_id).await {
                Ok(r) => r,
                Err(e) => {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "TAX_RULES".into(),
                            message: e.to_string(),
                            field: None,
                        }],
                    };
                }
            };
            let mut item_to_tax_category: std::collections::HashMap<Uuid, Uuid> =
                std::collections::HashMap::new();
            for line in &cart.lines {
                if let Ok(Some(item)) = get_catalog_item(pool, store_id, line.item_id).await {
                    item_to_tax_category.insert(line.item_id, item.tax_category_id);
                }
            }
            if let Err(errors) =
                apply_manual_discounts_to_lines(&mut cart, &rules, &item_to_tax_category)
            {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            cart.set_discounted();
            if let Err(errors) = save_cart_to_db(pool, &cart).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            let state = build_cart_state(pool, store_id, &cart).await;
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(cart_state_to_payload(&state)),
                errors: vec![],
            }
        }
        PosCommand::SetTendering(p) => {
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CART_NOT_FOUND".into(),
                        message: "Cart not found".into(),
                        field: None,
                    }],
                };
            };
            if cart.ensure_can_tender().is_err() {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_STATE".into(),
                        message: "Cart cannot enter tendering".into(),
                        field: None,
                    }],
                };
            }
            cart.set_tendering();
            if let Err(errors) = save_cart_to_db(pool, &cart).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            let state = build_cart_state(pool, store_id, &cart).await;
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(cart_state_to_payload(&state)),
                errors: vec![],
            }
        }
        PosCommand::AddPayment(p) => {
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CART_NOT_FOUND".into(),
                        message: "Cart not found".into(),
                        field: None,
                    }],
                };
            };
            if cart
                .add_payment(p.tender_id, p.amount_cents, p.external_reference.clone())
                .is_err()
            {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_PAYMENT".into(),
                        message: "Cannot add payment in current state".into(),
                        field: None,
                    }],
                };
            }
            if let Err(errors) = save_cart_to_db(pool, &cart).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            let state = build_cart_state(pool, store_id, &cart).await;
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(cart_state_to_payload(&state)),
                errors: vec![],
            }
        }
        PosCommand::FinalizeOrder(p) => {
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CART_NOT_FOUND".into(),
                        message: "Cart not found".into(),
                        field: None,
                    }],
                };
            };
            let order_id = Uuid::new_v4();
            let order = match cart.to_order(order_id) {
                Ok(o) => o,
                Err(_) => {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "FINALIZE_FAILED".into(),
                            message: "Cart must be paid and tendered >= total".into(),
                            field: None,
                        }],
                    };
                }
            };
            let hq_payload = order.to_hq_payload();
            let submission_id = Uuid::new_v4();
            let sequence_number = 1u64;
            let envelope_hq = build_submission_envelope(
                submission_id,
                store_id,
                register_id,
                sequence_number,
                hq_payload.clone(),
            );
            let envelope_json = serde_json::to_string(&envelope_hq).unwrap_or_default();
            if let Err(e) = insert_outbox(pool, submission_id, &envelope_json).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "OUTBOX_FAILED".into(),
                        message: e.to_string(),
                        field: None,
                    }],
                };
            }
            let doc_id = Uuid::new_v4();
            let template_id = Uuid::nil();
            let receipt_payload = serde_json::json!({
                "order_id": order_id.to_string(),
                "cart_id": cart.id.to_string(),
                "total_cents": order.total_cents,
                "subtotal_cents": order.subtotal_cents,
                "discount_cents": order.discount_cents,
                "tax_cents": order.tax_cents,
                "lines": order.lines.iter().map(|l| serde_json::json!({
                    "sku": l.sku,
                    "name": l.name,
                    "quantity": l.quantity,
                    "line_total_cents": l.line_total_cents,
                    "discount_cents": l.discount_cents,
                })).collect::<Vec<_>>(),
                "payments": order.payments.iter().map(|(tender_id, amount, _)| serde_json::json!({
                    "tender_id": tender_id.to_string(),
                    "amount_cents": amount
                })).collect::<Vec<_>>(),
            });
            let receipt_payload_str = receipt_payload.to_string();
            if let Err(e) = generate_document(
                pool,
                doc_id,
                "receipt",
                Some(order_id),
                Some(cart.id),
                template_id,
                "{{order_id}} Total: {{total_cents}}",
                &receipt_payload_str,
                "text/plain",
            )
            .await
            {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "DOCUMENT_FAILED".into(),
                        message: e.to_string(),
                        field: None,
                    }],
                };
            }
            cart.set_finalized();
            if let Err(errors) = save_cart_to_db(pool, &cart).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            let result = FinalizeResult {
                order_id,
                cart_id: cart.id,
                total_cents: order.total_cents,
                print_job_ids: vec![doc_id],
            };
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(finalize_result_to_payload(&result)),
                errors: vec![],
            }
        }
        PosCommand::RemoveLineItem(p) => {
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CART_NOT_FOUND".into(),
                        message: "Cart not found".into(),
                        field: None,
                    }],
                };
            };
            if cart.ensure_can_edit().is_err() {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_STATE".into(),
                        message: "Cart cannot be edited".into(),
                        field: None,
                    }],
                };
            }
            if let Err(e) = cart.remove_line_item(p.line_id) {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "LINE_NOT_FOUND".into(),
                        message: e.to_string(),
                        field: None,
                    }],
                };
            }
            if !cart.lines.is_empty() {
                if let Err(errors) = run_pricing_pipeline(pool, store_id, &mut cart).await {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors,
                    };
                }
            }
            if let Err(errors) = save_cart_to_db(pool, &cart).await {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            let state = build_cart_state(pool, store_id, &cart).await;
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(cart_state_to_payload(&state)),
                errors: vec![],
            }
        }
        _ => PosResponseEnvelope {
            version: ContractVersion::V1_0_0,
            success: false,
            idempotency_key,
            payload: None,
            errors: vec![PosError {
                code: "UNSUPPORTED_COMMAND".into(),
                message: "Command not yet implemented".into(),
                field: None,
            }],
        },
    };
    result
}
