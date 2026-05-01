//! POS command execution: load/save cart, run pricing pipeline, return payloads.

use apex_edge_contracts::{
    build_submission_envelope, AddPaymentInput, AppliedPromoInfo, CartState, CartStateKind,
    ContractVersion, FinalizeResult, ManualDiscountInfo, ManualDiscountKind, PosCommand, PosError,
    PosRequestEnvelope, PosResponseEnvelope, Promotion, PromotionType, TaxRule,
};
use apex_edge_domain::{
    apply_promos_with_attribution, base_price_cents, check_eligibility, tax_for_line, Cart,
    CartLineItem, LinePriceResult,
};
use apex_edge_printing::generate_document;
use apex_edge_storage::{
    fetch_open_shift, get_catalog_item, get_coupon_definition_by_code, get_customer,
    get_print_template, insert_order_ledger_entry, insert_outbox, insert_stock_movement,
    list_parked_carts, list_price_book_entries, list_promotions, list_tax_rules, load_cart,
    park_cart, recall_parked_cart, save_cart, NewOrderLedgerEntry, NewOrderLineEntry,
    NewOrderPaymentEntry, ParkCartInput, StockMovementInput,
};
use sqlx::SqlitePool;
use std::sync::OnceLock;
use std::time::Instant;
use uuid::Uuid;

use crate::pos::AppState;

fn finalize_timing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("APEX_EDGE_PROFILE_FINALIZE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

fn log_finalize_timing(event: &str, fields: &[(&str, String)]) {
    if !finalize_timing_enabled() {
        return;
    }
    let suffix = if fields.is_empty() {
        String::new()
    } else {
        format!(
            " {}",
            fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    eprintln!("[ApexEdge][Finalize] {event}{suffix}");
}

fn cart_state_to_payload(state: &CartState) -> serde_json::Value {
    serde_json::to_value(state).unwrap_or(serde_json::Value::Null)
}

/// Build a `CartState` from a `Cart`.
pub async fn build_cart_state(pool: &SqlitePool, store_id: Uuid, cart: &Cart) -> CartState {
    tracing::debug!(store_id = %store_id, pool_size = std::mem::size_of_val(pool), "building cart state");
    let mut state = cart.to_cart_state();
    if let Some(customer_id) = cart.customer_id {
        if let Ok(Some(customer)) = get_customer(pool, store_id, customer_id).await {
            state.customer_name = Some(customer.name);
            state.customer_code = Some(customer.code);
        }
    }
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

fn payment_tender_type(external_reference: &Option<String>) -> String {
    match external_reference.as_deref().map(str::trim) {
        Some(reference) if reference.eq_ignore_ascii_case("cash") => "cash".into(),
        Some(reference) if !reference.is_empty() => "external".into(),
        _ => "unknown".into(),
    }
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

/// Apply tax to each pricing result, returning `PRICING_INTERNAL` if a result references a line
/// that is not present in `cart_lines`. Extracted for testability.
pub(crate) fn apply_tax_to_pricing_results<F>(
    results: &mut [LinePriceResult],
    cart_lines: &[CartLineItem],
    category_by_item: &F,
    rules: &[TaxRule],
) -> Result<(), Vec<PosError>>
where
    F: Fn(Uuid) -> Uuid,
{
    for res in results.iter_mut() {
        let line = cart_lines
            .iter()
            .find(|l| l.line_id == res.line_id)
            .ok_or_else(|| {
                vec![PosError {
                    code: "PRICING_INTERNAL".into(),
                    message: "Pricing result references an unknown line".into(),
                    field: None,
                }]
            })?;
        let tax_cat = category_by_item(line.item_id);
        let line_net = res.line_total_cents.saturating_sub(res.discount_cents);
        res.tax_cents = tax_for_line(line_net, tax_cat, rules, false);
    }
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
    // Automatic promotions are promotions without a coupon code.
    let automatic_promos: Vec<Promotion> = promos
        .iter()
        .filter(|p| p.code.is_none())
        .cloned()
        .collect();
    let requested_manual_promo_ids: std::collections::HashSet<Uuid> =
        cart.applied_promo_ids.iter().copied().collect();
    let existing_auto_ids: std::collections::HashSet<Uuid> =
        automatic_promos.iter().map(|promo| promo.id).collect();
    let mut promos_to_apply: Vec<Promotion> = automatic_promos;
    promos_to_apply.extend(
        promos
            .iter()
            .filter(|p| requested_manual_promo_ids.contains(&p.id))
            .filter(|p| !existing_auto_ids.contains(&p.id))
            .cloned(),
    );
    let (mut results, applied_promo_ids) =
        apply_promos_with_attribution(&cart.lines, category_by_item, &promos_to_apply, subtotal);

    apply_tax_to_pricing_results(&mut results, &cart.lines, &category_by_item, &rules)?;

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
    apply_coupon_discounts(cart, &promos);
    if applied_any || !cart.manual_discounts.is_empty() {
        cart.set_discounted();
    }
    if cart.applied_coupons.iter().any(|c| c.discount_cents > 0) {
        cart.set_discounted();
    }
    Ok(())
}

fn coupon_discount_from_promo(promo_type: &PromotionType, basket_net_cents: u64) -> u64 {
    match promo_type {
        PromotionType::PercentageOff { percent_bps } => {
            basket_net_cents.saturating_mul(*percent_bps as u64) / 10000
        }
        PromotionType::FixedAmountOff { amount_cents } => (*amount_cents).min(basket_net_cents),
        PromotionType::BuyXGetY { .. } | PromotionType::PriceOverride { .. } => 0,
    }
}

/// Recompute coupon discounts based on currently applied coupons and active code-based promotions.
fn apply_coupon_discounts(cart: &mut Cart, promos: &[Promotion]) {
    if cart.applied_coupons.is_empty() {
        return;
    }
    let mut basket_net_cents: u64 = cart
        .lines
        .iter()
        .map(|l| l.line_total_cents.saturating_sub(l.discount_cents))
        .sum();

    for coupon in &mut cart.applied_coupons {
        coupon.discount_cents = 0;
        let Some(promo) = promos.iter().find(|p| {
            p.code
                .as_deref()
                .map(|c| c.eq_ignore_ascii_case(&coupon.code))
                .unwrap_or(false)
        }) else {
            continue;
        };
        let now = chrono::Utc::now();
        if now < promo.valid_from || promo.valid_until.map(|u| now > u).unwrap_or(false) {
            continue;
        }
        let discount = coupon_discount_from_promo(&promo.promo_type, basket_net_cents);
        coupon.coupon_id = promo.id;
        coupon.discount_cents = discount.min(basket_net_cents);
        basket_net_cents = basket_net_cents.saturating_sub(coupon.discount_cents);
    }
}

/// Apply stored manual discounts to lines (add amount to line.discount_cents) and recalc tax.
fn apply_manual_discounts_to_lines(
    cart: &mut Cart,
    rules: &[TaxRule],
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
        PosCommand::UpdateLineItem(p) => {
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
            if p.quantity == 0 {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_QUANTITY".into(),
                        message: "Quantity must be greater than zero".into(),
                        field: Some("quantity".into()),
                    }],
                };
            }
            let Some(line) = cart.lines.iter_mut().find(|l| l.line_id == p.line_id) else {
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
            };
            line.quantity = p.quantity;
            line.notes = p.notes.clone();
            line.line_total_cents = line.unit_price_cents.saturating_mul(line.quantity as u64);

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
        PosCommand::ApplyCoupon(p) => {
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
            let code = p.coupon_code.trim().to_uppercase();
            if code.is_empty() {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_COUPON".into(),
                        message: "Coupon code is required".into(),
                        field: Some("coupon_code".into()),
                    }],
                };
            }
            let Some(coupon_def) = get_coupon_definition_by_code(pool, store_id, &code)
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
                        code: "COUPON_NOT_FOUND".into(),
                        message: "Coupon not found".into(),
                        field: Some("coupon_code".into()),
                    }],
                };
            };
            let promos = match list_promotions(pool, store_id).await {
                Ok(p) => p,
                Err(e) => {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "PROMOTIONS".into(),
                            message: e.to_string(),
                            field: None,
                        }],
                    };
                }
            };
            let Some(promo) = promos.iter().find(|promo| promo.id == coupon_def.promo_id) else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "COUPON_NOT_FOUND".into(),
                        message: "Coupon not found".into(),
                        field: Some("coupon_code".into()),
                    }],
                };
            };
            let basket_subtotal = cart.subtotal_cents();
            let promo_discount_cents: u64 = cart.lines.iter().map(|line| line.discount_cents).sum();
            let eligibility = check_eligibility(
                &coupon_def,
                0,
                Some(0),
                basket_subtotal,
                promo_discount_cents,
            );
            if !eligibility.valid {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_COUPON".into(),
                        message: eligibility
                            .reason
                            .unwrap_or_else(|| "Coupon is not eligible".into()),
                        field: Some("coupon_code".into()),
                    }],
                };
            }
            let Some(promo_code) = promo.code.as_deref() else {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_COUPON".into(),
                        message: "Coupon promotion is missing code".into(),
                        field: Some("coupon_code".into()),
                    }],
                };
            };
            if !cart
                .applied_coupons
                .iter()
                .any(|c| c.code.eq_ignore_ascii_case(&code))
            {
                cart.applied_coupons
                    .push(apex_edge_domain::cart::AppliedCouponRecord {
                        coupon_id: coupon_def.id,
                        code: promo_code.to_string(),
                        discount_cents: 0,
                    });
            }

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
        PosCommand::RemoveCoupon(p) => {
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
            let before = cart.applied_coupons.len();
            cart.applied_coupons.retain(|c| c.coupon_id != p.coupon_id);
            if cart.applied_coupons.len() == before {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "COUPON_NOT_FOUND".into(),
                        message: "Coupon not found on cart".into(),
                        field: Some("coupon_id".into()),
                    }],
                };
            }

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
        PosCommand::ApplyPromo(p) => {
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
            let promo_exists = list_promotions(pool, store_id)
                .await
                .ok()
                .map(|promos| promos.into_iter().any(|promo| promo.id == p.promo_id))
                .unwrap_or(false);
            if !promo_exists {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "PROMO_NOT_FOUND".into(),
                        message: "Promotion not found".into(),
                        field: Some("promo_id".into()),
                    }],
                };
            }
            if !cart.applied_promo_ids.contains(&p.promo_id) {
                cart.applied_promo_ids.push(p.promo_id);
            }
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
        PosCommand::RemovePromo(p) => {
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
            let before = cart.applied_promo_ids.len();
            cart.applied_promo_ids.retain(|id| *id != p.promo_id);
            if cart.applied_promo_ids.len() == before {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "PROMO_NOT_FOUND".into(),
                        message: "Promotion not applied on cart".into(),
                        field: Some("promo_id".into()),
                    }],
                };
            }
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
            let payment_started_at = Instant::now();
            let payment_provider = p.provider.as_deref().unwrap_or("manual");
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                metrics::counter!(
                    apex_edge_metrics::PAYMENT_ATTEMPTS_TOTAL,
                    1u64,
                    "provider" => payment_provider.to_string(),
                    "outcome" => apex_edge_metrics::OUTCOME_ERROR
                );
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
                .add_payment(AddPaymentInput {
                    tender_id: p.tender_id,
                    amount_cents: p.amount_cents,
                    tip_amount_cents: p.tip_amount_cents,
                    external_reference: p.external_reference.clone(),
                    provider: p.provider.clone(),
                    provider_payment_id: p.provider_payment_id.clone(),
                    entry_method: p.entry_method,
                })
                .is_err()
            {
                metrics::counter!(
                    apex_edge_metrics::PAYMENT_ATTEMPTS_TOTAL,
                    1u64,
                    "provider" => payment_provider.to_string(),
                    "outcome" => apex_edge_metrics::OUTCOME_ERROR
                );
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
                metrics::counter!(
                    apex_edge_metrics::PAYMENT_ATTEMPTS_TOTAL,
                    1u64,
                    "provider" => payment_provider.to_string(),
                    "outcome" => apex_edge_metrics::OUTCOME_ERROR
                );
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            metrics::counter!(
                apex_edge_metrics::PAYMENT_ATTEMPTS_TOTAL,
                1u64,
                "provider" => payment_provider.to_string(),
                "outcome" => apex_edge_metrics::OUTCOME_SUCCESS
            );
            metrics::histogram!(
                apex_edge_metrics::PAYMENT_DURATION_SECONDS,
                payment_started_at.elapsed().as_secs_f64(),
                "provider" => payment_provider.to_string()
            );
            let state = build_cart_state(pool, store_id, &cart).await;
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(cart_state_to_payload(&state)),
                errors: vec![],
            }
        }
        PosCommand::VoidCart(p) => {
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
            if matches!(cart.state, CartStateKind::Finalized | CartStateKind::Voided) {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_STATE".into(),
                        message: "Cart cannot be voided in current state".into(),
                        field: None,
                    }],
                };
            }
            cart.lines.clear();
            cart.applied_promo_ids.clear();
            cart.applied_coupons.clear();
            cart.manual_discounts.clear();
            cart.payments.clear();
            cart.set_voided();
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
        PosCommand::ParkCart(p) => {
            let Some(cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
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
            let data = match serde_json::to_value(&cart) {
                Ok(data) => data,
                Err(e) => {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "CART_SERIALIZE".into(),
                            message: e.to_string(),
                            field: None,
                        }],
                    };
                }
            };
            let summary = match park_cart(
                pool,
                ParkCartInput {
                    cart_id: cart.id,
                    store_id,
                    register_id,
                    note: p.note.as_deref(),
                    cart_data: &data,
                    total_cents: cart.total_cents(),
                    line_count: cart.lines.len(),
                },
            )
            .await
            {
                Ok(summary) => summary,
                Err(e) => {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "PARK_CART_FAILED".into(),
                            message: e.to_string(),
                            field: None,
                        }],
                    };
                }
            };
            PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(serde_json::to_value(summary).unwrap_or(serde_json::Value::Null)),
                errors: vec![],
            }
        }
        PosCommand::RecallCart(p) => {
            let Some(data) = recall_parked_cart(pool, p.parked_cart_id)
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
                        code: "PARKED_CART_NOT_FOUND".into(),
                        message: "Parked cart not found".into(),
                        field: None,
                    }],
                };
            };
            let cart: Cart = match serde_json::from_value(data) {
                Ok(cart) => cart,
                Err(e) => {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "CART_DESERIALIZE".into(),
                            message: e.to_string(),
                            field: None,
                        }],
                    };
                }
            };
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
        PosCommand::ListParkedCarts(p) => match list_parked_carts(pool, store_id, p.register_id)
            .await
        {
            Ok(summaries) => PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: true,
                idempotency_key,
                payload: Some(serde_json::to_value(summaries).unwrap_or(serde_json::Value::Null)),
                errors: vec![],
            },
            Err(e) => PosResponseEnvelope {
                version: ContractVersion::V1_0_0,
                success: false,
                idempotency_key,
                payload: None,
                errors: vec![PosError {
                    code: "LIST_PARKED_CARTS_FAILED".into(),
                    message: e.to_string(),
                    field: None,
                }],
            },
        },
        PosCommand::ClockIn(p) => {
            match apex_edge_storage::clock_in(pool, store_id, register_id, p.associate_id.trim())
                .await
            {
                Ok(entry) => PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: true,
                    idempotency_key,
                    payload: Some(serde_json::to_value(entry).unwrap_or(serde_json::Value::Null)),
                    errors: vec![],
                },
                Err(e) => PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CLOCK_IN_FAILED".into(),
                        message: e.to_string(),
                        field: None,
                    }],
                },
            }
        }
        PosCommand::ClockOut(p) => {
            match apex_edge_storage::clock_out(pool, store_id, p.associate_id.trim()).await {
                Ok(Some(entry)) => PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: true,
                    idempotency_key,
                    payload: Some(serde_json::to_value(entry).unwrap_or(serde_json::Value::Null)),
                    errors: vec![],
                },
                Ok(None) => PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CLOCK_ENTRY_NOT_FOUND".into(),
                        message: "No open time clock entry found".into(),
                        field: Some("associate_id".into()),
                    }],
                },
                Err(e) => PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "CLOCK_OUT_FAILED".into(),
                        message: e.to_string(),
                        field: None,
                    }],
                },
            }
        }
        PosCommand::ReceiveStock(p) | PosCommand::TransferStock(p) | PosCommand::AdjustStock(p) => {
            let operation = match &envelope.payload {
                PosCommand::ReceiveStock(_) => "receive_stock",
                PosCommand::TransferStock(_) => "transfer_stock",
                PosCommand::AdjustStock(_) => "adjust_stock",
                _ => "stock_operation",
            };
            if p.quantity_delta == 0 || p.reason.trim().is_empty() {
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "INVALID_STOCK_MOVEMENT".into(),
                        message: "Stock movement requires a non-zero quantity and reason".into(),
                        field: None,
                    }],
                };
            }
            match insert_stock_movement(
                pool,
                StockMovementInput {
                    store_id,
                    register_id,
                    item_id: p.item_id,
                    operation,
                    quantity_delta: p.quantity_delta,
                    reason: p.reason.trim(),
                    reference: p.reference.as_deref(),
                },
            )
            .await
            {
                Ok(movement) => {
                    let payload = serde_json::to_string(&serde_json::json!({
                        "event_type": "stock.movement",
                        "movement": movement,
                    }))
                    .unwrap_or_default();
                    let _ = insert_outbox(pool, movement.id, &payload).await;
                    PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: true,
                        idempotency_key,
                        payload: Some(
                            serde_json::to_value(movement).unwrap_or(serde_json::Value::Null),
                        ),
                        errors: vec![],
                    }
                }
                Err(e) => PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "STOCK_MOVEMENT_FAILED".into(),
                        message: e.to_string(),
                        field: None,
                    }],
                },
            }
        }
        PosCommand::FinalizeOrder(p) => {
            let finalize_started_at = Instant::now();
            log_finalize_timing("start", &[("cart_id", p.cart_id.to_string())]);
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
            let shift_id = fetch_open_shift(pool, store_id, register_id)
                .await
                .ok()
                .flatten()
                .map(|shift| shift.id);
            let ledger_entry = NewOrderLedgerEntry {
                order_id,
                cart_id: cart.id,
                store_id,
                register_id,
                shift_id,
                subtotal_cents: order.subtotal_cents,
                discount_cents: order.discount_cents,
                tax_cents: order.tax_cents,
                total_cents: order.total_cents,
                submission_id: Some(submission_id),
                lines: order
                    .lines
                    .iter()
                    .map(|line| NewOrderLineEntry {
                        line_id: line.line_id,
                        item_id: line.item_id,
                        sku: line.sku.clone(),
                        name: line.name.clone(),
                        quantity: line.quantity,
                        unit_price_cents: line.unit_price_cents,
                        line_total_cents: line.line_total_cents,
                        discount_cents: line.discount_cents,
                        tax_cents: line.tax_cents,
                    })
                    .collect(),
                payments: order
                    .payments
                    .iter()
                    .map(|payment| NewOrderPaymentEntry {
                        tender_id: payment.tender_id,
                        tender_type: payment_tender_type(&payment.external_reference),
                        amount_cents: payment.amount_cents,
                        tip_amount_cents: payment.tip_amount_cents,
                        external_reference: payment.external_reference.clone(),
                        provider: payment.provider.clone(),
                        provider_payment_id: payment.provider_payment_id.clone(),
                        entry_method: payment.entry_method,
                    })
                    .collect(),
            };
            let ledger_started_at = Instant::now();
            if let Err(e) = insert_order_ledger_entry(pool, &ledger_entry).await {
                metrics::counter!(
                    apex_edge_metrics::ORDERS_FINALIZED_TOTAL,
                    1u64,
                    "outcome" => apex_edge_metrics::OUTCOME_ERROR
                );
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "ORDER_LEDGER_FAILED".into(),
                        message: e.to_string(),
                        field: None,
                    }],
                };
            }
            metrics::counter!(
                apex_edge_metrics::ORDERS_FINALIZED_TOTAL,
                1u64,
                "outcome" => apex_edge_metrics::OUTCOME_SUCCESS
            );
            metrics::histogram!(
                apex_edge_metrics::ORDERS_LEDGER_WRITE_DURATION_SECONDS,
                ledger_started_at.elapsed().as_secs_f64()
            );
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
            log_finalize_timing(
                "outbox_inserted",
                &[
                    (
                        "elapsed_ms",
                        finalize_started_at.elapsed().as_millis().to_string(),
                    ),
                    ("submission_id", submission_id.to_string()),
                ],
            );
            let doc_id = Uuid::new_v4();
            let (customer_name, customer_address) = match cart.customer_id {
                Some(cid) => {
                    if let Ok(Some(c)) = get_customer(pool, store_id, cid).await {
                        (
                            Some(c.name),
                            c.email.as_ref().map(|e| format!("Email: {e}")),
                        )
                    } else {
                        (None, None)
                    }
                }
                None => (None, None),
            };
            let receipt_payload = serde_json::json!({
                "order_id": order_id.to_string(),
                "cart_id": cart.id.to_string(),
                "total_cents": order.total_cents,
                "subtotal_cents": order.subtotal_cents,
                "discount_cents": order.discount_cents,
                "tax_cents": order.tax_cents,
                "store_name": "Store",
                "store_address": "",
                "customer_name": customer_name.unwrap_or_default(),
                "customer_address": customer_address.unwrap_or_default(),
                "tenant": "Tenant",
                "logo_placeholder": "",
                "created_at": order.created_at.to_rfc3339(),
                "lines": order.lines.iter().map(|l| serde_json::json!({
                    "sku": l.sku,
                    "name": l.name,
                    "quantity": l.quantity,
                    "unit_price_cents": l.unit_price_cents,
                    "line_total_cents": l.line_total_cents,
                    "discount_cents": l.discount_cents,
                    "tax_cents": l.tax_cents,
                })).collect::<Vec<_>>(),
                "payments": order.payments.iter().map(|payment| serde_json::json!({
                    "tender_id": payment.tender_id.to_string(),
                    "amount_cents": payment.amount_cents,
                    "tip_amount_cents": payment.tip_amount_cents,
                    "provider": payment.provider.clone(),
                    "provider_payment_id": payment.provider_payment_id.clone(),
                    "entry_method": payment.entry_method,
                })).collect::<Vec<_>>(),
            });
            let receipt_payload_str = receipt_payload.to_string();

            let template = get_print_template(pool, store_id, "customer_receipt")
                .await
                .ok()
                .flatten();
            let (doc_type, template_id, template_body, mime_type) = if let Some(ref t) = template {
                (
                    "customer_receipt",
                    t.template_id,
                    t.template_body.as_str(),
                    "application/pdf",
                )
            } else {
                (
                    "receipt",
                    Uuid::nil(),
                    "{{order_id}} Total: {{total_cents}}",
                    "text/plain",
                )
            };

            if let Err(e) = generate_document(
                pool,
                doc_id,
                doc_type,
                Some(order_id),
                Some(cart.id),
                template_id,
                template_body,
                &receipt_payload_str,
                mime_type,
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
            log_finalize_timing(
                "document_generated",
                &[
                    (
                        "elapsed_ms",
                        finalize_started_at.elapsed().as_millis().to_string(),
                    ),
                    ("doc_id", doc_id.to_string()),
                ],
            );
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
            log_finalize_timing(
                "cart_saved",
                &[(
                    "elapsed_ms",
                    finalize_started_at.elapsed().as_millis().to_string(),
                )],
            );
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
        PosCommand::StartReturn(p) => {
            return crate::returns_handler::start_return(
                app,
                store_id,
                register_id,
                idempotency_key,
                p,
            )
            .await;
        }
        PosCommand::ReturnLineItem(p) => {
            return crate::returns_handler::return_line_item(app, store_id, idempotency_key, p)
                .await;
        }
        PosCommand::RefundTender(p) => {
            return crate::returns_handler::refund_tender(app, store_id, idempotency_key, p).await;
        }
        PosCommand::FinalizeReturn(p) => {
            return crate::returns_handler::finalize_return(
                app,
                store_id,
                register_id,
                idempotency_key,
                p,
            )
            .await;
        }
        PosCommand::VoidReturn(p) => {
            return crate::returns_handler::void_return(app, store_id, idempotency_key, p).await;
        }
        PosCommand::OpenTill(p) => {
            return crate::shifts_handler::open_till(
                app,
                store_id,
                register_id,
                idempotency_key,
                p,
            )
            .await;
        }
        PosCommand::PaidIn(p) => {
            return crate::shifts_handler::paid_in(app, store_id, idempotency_key, p).await;
        }
        PosCommand::PaidOut(p) => {
            return crate::shifts_handler::paid_out(app, store_id, idempotency_key, p).await;
        }
        PosCommand::NoSale(p) => {
            return crate::shifts_handler::no_sale(app, store_id, idempotency_key, p).await;
        }
        PosCommand::CashCount(p) => {
            return crate::shifts_handler::cash_count(app, store_id, idempotency_key, p).await;
        }
        PosCommand::GetXReport(p) => {
            return crate::shifts_handler::get_x_report(app, store_id, idempotency_key, p).await;
        }
        PosCommand::CloseTill(p) => {
            return crate::shifts_handler::close_till(
                app,
                store_id,
                register_id,
                idempotency_key,
                p,
            )
            .await;
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
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn apply_tax_returns_pricing_internal_error_for_mismatched_line_id() {
        let item_id = Uuid::new_v4();
        let real_line_id = Uuid::new_v4();
        let phantom_line_id = Uuid::new_v4();

        let cart_lines = vec![CartLineItem {
            line_id: real_line_id,
            item_id,
            sku: "SKU-001".into(),
            name: "Test Item".into(),
            quantity: 1,
            modifier_option_ids: vec![],
            notes: None,
            unit_price_cents: 1000,
            line_total_cents: 1000,
            discount_cents: 0,
            tax_cents: 0,
        }];

        // A result whose line_id is NOT present in cart_lines — the invariant-violation case.
        let mut results = vec![LinePriceResult {
            line_id: phantom_line_id,
            unit_price_cents: 1000,
            line_total_cents: 1000,
            discount_cents: 0,
            tax_cents: 0,
        }];

        let no_tax = |_: Uuid| Uuid::nil();
        let rules = vec![];

        let err = apply_tax_to_pricing_results(&mut results, &cart_lines, &no_tax, &rules)
            .expect_err("must return PRICING_INTERNAL when result line_id is not in cart");

        assert_eq!(err.len(), 1);
        assert_eq!(err[0].code, "PRICING_INTERNAL");
    }
}
