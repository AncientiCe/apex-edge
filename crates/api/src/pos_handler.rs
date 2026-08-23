//! POS command execution: load/save cart, run pricing pipeline, return payloads.

use apex_edge_adapters_payment::{
    AuthorizationOutcome, AuthorizeRequest, CaptureRequest, PaymentProviderError,
};
use apex_edge_contracts::{
    build_submission_envelope, AddPaymentInput, AppliedPromoInfo, CartState, CartStateKind,
    ContractVersion, FinalizeResult, ManualDiscountInfo, ManualDiscountKind, PosCommand, PosError,
    PosRequestEnvelope, PosResponseEnvelope, Promotion, PromotionType, TaxRule,
};
use apex_edge_domain::{
    apply_promos_with_attribution, base_price_cents, check_eligibility, tax_for_line, Cart,
    CartLineItem, LinePriceResult,
};
use apex_edge_loyalty::{EarnRequest, LocalLoyaltyProvider, LoyaltyAccount, LoyaltyProvider};
use apex_edge_printing::generate_document;
use apex_edge_storage::{
    activate_gift_card, apply_local_delta, claim_parked_cart, commit_cart_sale,
    earn_loyalty_points, ensure_inventory_state, fetch_open_shift,
    flag_payment_intent_for_reversal, flag_payment_intents_for_reversal, get_catalog_item,
    get_coupon_definition_by_code, get_customer, get_print_template, insert_order_ledger_entry,
    insert_outbox, insert_payment_intent, insert_stock_movement, issue_gift_card,
    list_parked_carts, list_price_book_entries, list_promotions, list_tax_rules, load_cart,
    mark_payment_intent_captured, mark_payment_intent_declined, mark_payment_intent_failed,
    park_cart, redeem_gift_card, redeem_loyalty_points, release_cart_reservations,
    release_line_reservation, reload_gift_card, save_cart, settle_payment_intents_for_cart,
    try_reserve, ActivateOutcome, ClaimOutcome, IssueOutcome, NewOrderLedgerEntry,
    NewOrderLineEntry, NewOrderPaymentEntry, NewPaymentIntent, ParkCartInput, PaymentIntentState,
    RedeemOutcome, RedeemPointsOutcome, ReloadOutcome, ReserveInput, ReserveOutcome,
    StockMovementInput,
};
use sqlx::SqlitePool;
use std::sync::OnceLock;
use std::time::Instant;
use uuid::Uuid;

use crate::inventory_realtime::{
    broadcast_stock_changed, record_oversell_prevented, record_reservation_outcome,
};
use crate::payments::{provider_idempotency_key, ProviderChoice, MANUAL_PROVIDER};
use crate::pos::AppState;
use crate::stream::{stream_broadcast, StreamKind};

/// Reservation time-to-live. Abandoned carts release their held stock after this window.
/// Configurable via `APEX_EDGE_RESERVATION_TTL_SECONDS` (default 3600s = 1h).
fn reservation_ttl_seconds() -> i64 {
    static TTL: OnceLock<i64> = OnceLock::new();
    *TTL.get_or_init(|| {
        std::env::var("APEX_EDGE_RESERVATION_TTL_SECONDS")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(3600)
    })
}

fn reservation_expiry() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() + chrono::Duration::seconds(reservation_ttl_seconds())
}

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

/// Everything the payment path needs to identify one tender attempt.
struct TakePaymentContext {
    idempotency_key: Uuid,
    store_id: Uuid,
    register_id: Uuid,
    cart_id: Uuid,
    tender_id: Uuid,
    amount_cents: u64,
    tip_amount_cents: u64,
    requested_provider: Option<String>,
}

/// A tender the store actually holds, ready to be recorded on the cart.
struct CapturedTender {
    /// `None` for manual tenders, which contact no provider.
    intent_id: Option<Uuid>,
    provider: Option<String>,
    provider_payment_id: Option<String>,
    entry_method: Option<apex_edge_contracts::PaymentEntryMethod>,
    /// What the provider approved, which may be less than what was asked for.
    amount_cents: u64,
    tip_amount_cents: u64,
    metric_outcome: &'static str,
}

struct PaymentFailure {
    code: &'static str,
    message: String,
    metric_outcome: &'static str,
}

/// Obtain funds for one tender.
///
/// Manual tenders (no provider named) are passed straight through, preserving the
/// behaviour cash, gift cards, and externally captured cards rely on. Provider-backed
/// tenders authorize, then capture, writing a durable intent at each step so a crash
/// leaves evidence of money that may have moved.
async fn take_payment(
    app: &AppState,
    ctx: &TakePaymentContext,
) -> Result<CapturedTender, PaymentFailure> {
    let provider = match app.payments.resolve(ctx.requested_provider.as_deref()) {
        ProviderChoice::ManualTender => {
            return Ok(CapturedTender {
                intent_id: None,
                provider: ctx.requested_provider.clone(),
                provider_payment_id: None,
                entry_method: None,
                amount_cents: ctx.amount_cents,
                tip_amount_cents: ctx.tip_amount_cents,
                metric_outcome: apex_edge_metrics::OUTCOME_SUCCESS,
            });
        }
        ProviderChoice::Unknown(code) => {
            return Err(PaymentFailure {
                code: "PAYMENT_PROVIDER_UNKNOWN",
                message: format!("Payment provider '{code}' is not configured on this hub"),
                metric_outcome: apex_edge_metrics::OUTCOME_UNKNOWN_PROVIDER,
            });
        }
        ProviderChoice::Provider(provider) => provider,
    };

    let intent = insert_payment_intent(
        &app.pool,
        NewPaymentIntent {
            store_id: ctx.store_id,
            register_id: ctx.register_id,
            cart_id: ctx.cart_id,
            tender_id: ctx.tender_id,
            idempotency_key: ctx.idempotency_key,
            provider: provider.provider_code().to_string(),
            amount_cents: ctx.amount_cents,
            tip_amount_cents: ctx.tip_amount_cents,
        },
    )
    .await
    .map_err(|e| PaymentFailure {
        code: "PAYMENT_LEDGER_UNAVAILABLE",
        message: format!("Could not record the payment attempt: {e}"),
        metric_outcome: apex_edge_metrics::OUTCOME_ERROR,
    })?;

    // A retried command finds its intent already captured. Answer from the ledger rather
    // than asking the provider for money a second time.
    if intent.state == PaymentIntentState::Captured || intent.state == PaymentIntentState::Settled {
        return Ok(CapturedTender {
            intent_id: Some(intent.id),
            provider: Some(intent.provider.clone()),
            provider_payment_id: intent.provider_payment_id.clone(),
            entry_method: None,
            amount_cents: intent.approved_cents,
            tip_amount_cents: intent.tip_amount_cents,
            metric_outcome: apex_edge_metrics::OUTCOME_SUCCESS,
        });
    }

    let authorization = provider
        .authorize(AuthorizeRequest {
            idempotency_key: provider_idempotency_key(
                ctx.idempotency_key,
                ctx.tender_id,
                "authorize",
            ),
            cart_id: ctx.cart_id,
            store_id: ctx.store_id,
            register_id: ctx.register_id,
            amount_cents: ctx.amount_cents,
            tip_amount_cents: ctx.tip_amount_cents,
            currency: app.payments.currency.clone(),
        })
        .await;

    let authorization = match authorization {
        Ok(authorization) => authorization,
        Err(e) if e.requires_reversal_check() => {
            // The provider may have taken the card. Queue it for the sweeper instead of
            // telling the operator the payment simply failed.
            let _ = flag_payment_intent_for_reversal(
                &app.pool,
                intent.id,
                indeterminate_payment_id(&e),
                &e.to_string(),
            )
            .await;
            return Err(PaymentFailure {
                code: "PAYMENT_INDETERMINATE",
                message: format!(
                    "The terminal did not confirm the payment and it is being reconciled: {e}"
                ),
                metric_outcome: apex_edge_metrics::OUTCOME_INDETERMINATE,
            });
        }
        Err(e) => {
            let _ = mark_payment_intent_failed(&app.pool, intent.id, &e.to_string()).await;
            return Err(PaymentFailure {
                code: "PAYMENT_FAILED",
                message: e.to_string(),
                metric_outcome: apex_edge_metrics::OUTCOME_ERROR,
            });
        }
    };

    if !authorization.is_approved() {
        let (code, message) = authorization
            .declined_reason()
            .unwrap_or(("declined", "The payment was declined."));
        let _ = mark_payment_intent_declined(&app.pool, intent.id, code).await;
        return Err(PaymentFailure {
            code: "PAYMENT_DECLINED",
            message: message.to_string(),
            metric_outcome: apex_edge_metrics::OUTCOME_DECLINED,
        });
    }

    let partial = matches!(
        authorization.outcome,
        AuthorizationOutcome::PartiallyApproved { .. }
    );

    let capture = provider
        .capture(CaptureRequest {
            idempotency_key: provider_idempotency_key(
                ctx.idempotency_key,
                ctx.tender_id,
                "capture",
            ),
            provider_payment_id: authorization.provider_payment_id.clone(),
            amount_cents: authorization.approved_cents,
        })
        .await;

    let capture = match capture {
        Ok(capture) => capture,
        Err(e) => {
            metrics::counter!(apex_edge_metrics::PAYMENT_CAPTURES_TOTAL, "provider" => provider.provider_code(), "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
            if e.requires_reversal_check() {
                let _ = flag_payment_intent_for_reversal(
                    &app.pool,
                    intent.id,
                    Some(authorization.provider_payment_id.as_str()),
                    &e.to_string(),
                )
                .await;
                return Err(PaymentFailure {
                    code: "PAYMENT_INDETERMINATE",
                    message: format!("The capture did not confirm and is being reconciled: {e}"),
                    metric_outcome: apex_edge_metrics::OUTCOME_INDETERMINATE,
                });
            }
            // The authorization was never captured, so nothing is owed back; it will
            // expire on the provider side.
            let _ = mark_payment_intent_failed(&app.pool, intent.id, &e.to_string()).await;
            return Err(PaymentFailure {
                code: "PAYMENT_FAILED",
                message: e.to_string(),
                metric_outcome: apex_edge_metrics::OUTCOME_ERROR,
            });
        }
    };

    metrics::counter!(apex_edge_metrics::PAYMENT_CAPTURES_TOTAL, "provider" => provider.provider_code(), "outcome" => apex_edge_metrics::OUTCOME_SUCCESS).increment(1);

    if let Err(e) = mark_payment_intent_captured(
        &app.pool,
        intent.id,
        &capture.provider_payment_id,
        capture.captured_cents,
    )
    .await
    {
        // The money is taken but we failed to write that down. Log loudly: this is the
        // one case the sweeper cannot recover from on its own.
        tracing::error!(
            intent_id = %intent.id,
            provider_payment_id = %capture.provider_payment_id,
            error = %e,
            "captured a payment but could not record the capture"
        );
    }

    Ok(CapturedTender {
        intent_id: Some(intent.id),
        provider: Some(capture.provider),
        provider_payment_id: Some(capture.provider_payment_id),
        entry_method: capture.receipt.entry_method,
        amount_cents: capture.captured_cents,
        tip_amount_cents: authorization.tip_amount_cents,
        metric_outcome: if partial {
            apex_edge_metrics::OUTCOME_PARTIAL
        } else {
            apex_edge_metrics::OUTCOME_SUCCESS
        },
    })
}

fn indeterminate_payment_id(error: &PaymentProviderError) -> Option<&str> {
    match error {
        PaymentProviderError::Indeterminate {
            provider_payment_id,
            ..
        } => provider_payment_id.as_deref(),
        _ => None,
    }
}

/// Hand every captured payment on a cart to the reversal sweeper, because the sale the
/// customer paid for did not happen.
async fn flag_cart_payments_for_reversal(
    pool: &SqlitePool,
    store_id: Uuid,
    cart_id: Uuid,
    reason: &str,
) {
    match flag_payment_intents_for_reversal(pool, store_id, cart_id, reason).await {
        Ok(0) => {}
        Ok(flagged) => tracing::warn!(
            cart_id = %cart_id,
            flagged,
            reason,
            "flagged captured payments for reversal after a failed sale"
        ),
        Err(e) => {
            tracing::error!(cart_id = %cart_id, error = %e, "could not flag captured payments for reversal")
        }
    }
}

/// Hand a captured tender to the reversal sweeper. Manual tenders have no provider to
/// give money back, so they are skipped.
async fn flag_captured_tender_for_reversal(
    pool: &SqlitePool,
    tender: &CapturedTender,
    reason: &str,
) {
    let Some(intent_id) = tender.intent_id else {
        return;
    };
    if let Err(e) = flag_payment_intent_for_reversal(
        pool,
        intent_id,
        tender.provider_payment_id.as_deref(),
        reason,
    )
    .await
    {
        tracing::error!(intent_id = %intent_id, error = %e, "could not flag a captured payment for reversal");
    }
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
        Some(reference) if reference.starts_with("gift_card:") => "gift_card".into(),
        Some(reference) if !reference.is_empty() => "external".into(),
        _ => "unknown".into(),
    }
}

fn gift_card_error(
    idempotency_key: Uuid,
    code: &str,
    message: impl Into<String>,
) -> PosResponseEnvelope<serde_json::Value> {
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: false,
        idempotency_key,
        payload: None,
        errors: vec![PosError {
            code: code.into(),
            message: message.into(),
            field: None,
        }],
    }
}

fn gift_card_state_kind(
    state: &apex_edge_giftcards::GiftCardState,
) -> apex_edge_contracts::GiftCardStateKind {
    match state {
        apex_edge_giftcards::GiftCardState::Issued => {
            apex_edge_contracts::GiftCardStateKind::Issued
        }
        apex_edge_giftcards::GiftCardState::Active => {
            apex_edge_contracts::GiftCardStateKind::Active
        }
        apex_edge_giftcards::GiftCardState::Disabled => {
            apex_edge_contracts::GiftCardStateKind::Disabled
        }
    }
}

fn gift_card_info_payload(record: &apex_edge_storage::GiftCardRecord) -> serde_json::Value {
    serde_json::to_value(apex_edge_contracts::GiftCardInfo {
        gift_card_id: record.id,
        code: record.code.clone(),
        balance_cents: record.balance_cents,
        currency: record.currency.clone(),
        state: gift_card_state_kind(&record.state),
    })
    .unwrap_or(serde_json::Value::Null)
}

/// Generates a gift card code when the caller doesn't supply one. Not a formal check digit
/// scheme — just enough entropy to avoid collisions; `issue_gift_card`'s `UNIQUE(code)`
/// constraint is the actual duplicate guard.
fn generate_gift_card_code() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("GC-{}", hex[..12].to_uppercase())
}

/// Local loyalty conversion rates. Configurable via `APEX_EDGE_LOYALTY_CENTS_PER_POINT`
/// (default 100 = $1 spent earns 1 point) and `APEX_EDGE_LOYALTY_CENTS_PER_REDEEMED_POINT`
/// (default 1 = 1 point redeems for 1 cent). Read once per process, mirroring
/// `reservation_ttl_seconds` above.
fn loyalty_provider() -> &'static LocalLoyaltyProvider {
    static PROVIDER: OnceLock<LocalLoyaltyProvider> = OnceLock::new();
    PROVIDER.get_or_init(|| {
        let cents_per_point = std::env::var("APEX_EDGE_LOYALTY_CENTS_PER_POINT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(100);
        let cents_per_redeemed_point = std::env::var("APEX_EDGE_LOYALTY_CENTS_PER_REDEEMED_POINT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(1);
        LocalLoyaltyProvider::new(cents_per_point, cents_per_redeemed_point)
    })
}

fn loyalty_error(
    idempotency_key: Uuid,
    code: &str,
    message: impl Into<String>,
) -> PosResponseEnvelope<serde_json::Value> {
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: false,
        idempotency_key,
        payload: None,
        errors: vec![PosError {
            code: code.into(),
            message: message.into(),
            field: None,
        }],
    }
}

fn loyalty_info_payload(record: &apex_edge_storage::LoyaltyAccountRecord) -> serde_json::Value {
    serde_json::to_value(apex_edge_contracts::LoyaltyAccountInfo {
        customer_id: record.customer_id,
        points: record.points,
    })
    .unwrap_or(serde_json::Value::Null)
}

/// Computes the value (in cents) of redeeming `points`, via the same conversion trait used
/// for real redemptions. `redeem_loyalty_points` (storage) already atomically validated and
/// deducted a sufficient balance; this scratch account (seeded with exactly the redeemed
/// point count) exists only so the conversion math flows through the injected
/// `LoyaltyProvider` trait rather than being duplicated here.
fn loyalty_redeem_value_cents(customer_id: Uuid, points: u64) -> u64 {
    let mut scratch = LoyaltyAccount {
        customer_id,
        points,
    };
    loyalty_provider()
        .redeem(
            &mut scratch,
            apex_edge_loyalty::RedeemRequest {
                customer_id,
                points,
            },
        )
        .unwrap_or(0)
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

    if store_id != app.store_id {
        return PosResponseEnvelope {
            version: ContractVersion::V1_0_0,
            success: false,
            idempotency_key,
            payload: None,
            errors: vec![apex_edge_contracts::PosError {
                code: "STORE_MISMATCH".into(),
                message: "store_id does not match this hub".into(),
                field: Some("store_id".into()),
            }],
        };
    }

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
            // Inactive items are never sellable, regardless of tracked stock.
            if !item.is_active {
                metrics::counter!(apex_edge_metrics::CATALOG_STOCK_CHECKS_TOTAL, "outcome" => "OUT_OF_STOCK").increment(1);
                record_reservation_outcome("insufficient");
                record_oversell_prevented();
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors: vec![PosError {
                        code: "OUT_OF_STOCK".into(),
                        message: "Item is out of stock".into(),
                        field: None,
                    }],
                };
            }
            // Real-time reservation: atomically hold stock so concurrent registers cannot
            // oversell. Tracked items are lazily seeded from their synced baseline so the
            // guard holds even before startup/sync seeding has run.
            let line_id = Uuid::new_v4();
            if let Some(baseline) = item.available_qty {
                if let Err(e) = ensure_inventory_state(pool, store_id, p.item_id, baseline).await {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "INVENTORY_LEDGER_FAILED".into(),
                            message: e.to_string(),
                            field: None,
                        }],
                    };
                }
            }
            let reserve_outcome = try_reserve(
                pool,
                ReserveInput {
                    store_id,
                    register_id,
                    cart_id: p.cart_id,
                    line_id,
                    item_id: p.item_id,
                    qty: p.quantity as i64,
                    expires_at: Some(reservation_expiry()),
                },
            )
            .await;
            match reserve_outcome {
                Ok(ReserveOutcome::Reserved) => {
                    record_reservation_outcome("reserved");
                    metrics::counter!(apex_edge_metrics::CATALOG_STOCK_CHECKS_TOTAL, "outcome" => "ok").increment(1);
                }
                Ok(ReserveOutcome::Untracked) => {
                    record_reservation_outcome("untracked");
                    metrics::counter!(apex_edge_metrics::CATALOG_STOCK_CHECKS_TOTAL, "outcome" => "ok").increment(1);
                }
                Ok(ReserveOutcome::Insufficient { available }) => {
                    record_reservation_outcome("insufficient");
                    record_oversell_prevented();
                    let (code, message) = if available <= 0 {
                        ("OUT_OF_STOCK", "Item is out of stock".to_string())
                    } else {
                        (
                            "INSUFFICIENT_STOCK",
                            format!(
                                "Requested quantity {} exceeds available stock ({available})",
                                p.quantity
                            ),
                        )
                    };
                    metrics::counter!(apex_edge_metrics::CATALOG_STOCK_CHECKS_TOTAL, "outcome" => code).increment(1);
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: code.into(),
                            message,
                            field: None,
                        }],
                    };
                }
                Err(e) => {
                    record_reservation_outcome("error");
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "INVENTORY_LEDGER_FAILED".into(),
                            message: e.to_string(),
                            field: None,
                        }],
                    };
                }
            }
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
                let _ = release_line_reservation(pool, store_id, line_id).await;
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            if let Err(errors) = save_cart_to_db(pool, &cart).await {
                let _ = release_line_reservation(pool, store_id, line_id).await;
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            broadcast_stock_changed(app, store_id, &[p.item_id]).await;
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
            let Some((line_item_id, old_qty)) = cart
                .lines
                .iter()
                .find(|l| l.line_id == p.line_id)
                .map(|l| (l.item_id, l.quantity))
            else {
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
            // Re-reserve the line at the new quantity: release the current hold, then take
            // a fresh reservation. If the increase cannot be satisfied, restore the prior
            // hold and fail without mutating the cart.
            if p.quantity != old_qty {
                if let Err(e) = release_line_reservation(pool, store_id, p.line_id).await {
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "INVENTORY_LEDGER_FAILED".into(),
                            message: e.to_string(),
                            field: None,
                        }],
                    };
                }
                let outcome = try_reserve(
                    pool,
                    ReserveInput {
                        store_id,
                        register_id,
                        cart_id: p.cart_id,
                        line_id: p.line_id,
                        item_id: line_item_id,
                        qty: p.quantity as i64,
                        expires_at: Some(reservation_expiry()),
                    },
                )
                .await;
                match outcome {
                    Ok(ReserveOutcome::Reserved) => record_reservation_outcome("reserved"),
                    Ok(ReserveOutcome::Untracked) => record_reservation_outcome("untracked"),
                    Ok(ReserveOutcome::Insufficient { available }) => {
                        record_reservation_outcome("insufficient");
                        record_oversell_prevented();
                        // Restore the previous reservation so the cart is unchanged.
                        let _ = try_reserve(
                            pool,
                            ReserveInput {
                                store_id,
                                register_id,
                                cart_id: p.cart_id,
                                line_id: p.line_id,
                                item_id: line_item_id,
                                qty: old_qty as i64,
                                expires_at: Some(reservation_expiry()),
                            },
                        )
                        .await;
                        let (code, message) = if available <= 0 {
                            ("OUT_OF_STOCK", "Item is out of stock".to_string())
                        } else {
                            (
                                "INSUFFICIENT_STOCK",
                                format!(
                                    "Requested quantity {} exceeds available stock ({available})",
                                    p.quantity
                                ),
                            )
                        };
                        return PosResponseEnvelope {
                            version: ContractVersion::V1_0_0,
                            success: false,
                            idempotency_key,
                            payload: None,
                            errors: vec![PosError {
                                code: code.into(),
                                message,
                                field: None,
                            }],
                        };
                    }
                    Err(e) => {
                        record_reservation_outcome("error");
                        return PosResponseEnvelope {
                            version: ContractVersion::V1_0_0,
                            success: false,
                            idempotency_key,
                            payload: None,
                            errors: vec![PosError {
                                code: "INVENTORY_LEDGER_FAILED".into(),
                                message: e.to_string(),
                                field: None,
                            }],
                        };
                    }
                }
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
            broadcast_stock_changed(app, store_id, &[line_item_id]).await;
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
            let payment_provider = p.provider.as_deref().unwrap_or(MANUAL_PROVIDER);
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                metrics::counter!(apex_edge_metrics::PAYMENT_ATTEMPTS_TOTAL, "provider" => payment_provider.to_string(), "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
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

            // Ask the provider for the money before recording anything on the cart, so a
            // decline can never leave a tender the store did not actually receive.
            let tender = match take_payment(
                app,
                &TakePaymentContext {
                    idempotency_key,
                    store_id,
                    register_id,
                    cart_id: p.cart_id,
                    tender_id: p.tender_id,
                    amount_cents: p.amount_cents,
                    tip_amount_cents: p.tip_amount_cents,
                    requested_provider: p.provider.clone(),
                },
            )
            .await
            {
                Ok(tender) => tender,
                Err(error) => {
                    metrics::counter!(apex_edge_metrics::PAYMENT_ATTEMPTS_TOTAL, "provider" => payment_provider.to_string(), "outcome" => error.metric_outcome).increment(1);
                    metrics::histogram!(apex_edge_metrics::PAYMENT_DURATION_SECONDS, "provider" => payment_provider.to_string()).record(payment_started_at.elapsed().as_secs_f64());
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: error.code.into(),
                            message: error.message,
                            field: None,
                        }],
                    };
                }
            };

            if cart
                .add_payment(AddPaymentInput {
                    tender_id: p.tender_id,
                    amount_cents: tender.amount_cents,
                    tip_amount_cents: tender.tip_amount_cents,
                    external_reference: p.external_reference.clone(),
                    provider: tender.provider.clone(),
                    provider_payment_id: tender
                        .provider_payment_id
                        .clone()
                        .or_else(|| p.provider_payment_id.clone()),
                    entry_method: tender.entry_method.or(p.entry_method),
                })
                .is_err()
            {
                // The provider already has the money but the cart will not accept it, so
                // this is exactly the case the reversal sweeper exists for.
                flag_captured_tender_for_reversal(
                    pool,
                    &tender,
                    "cart rejected the tender after capture",
                )
                .await;
                metrics::counter!(apex_edge_metrics::PAYMENT_ATTEMPTS_TOTAL, "provider" => payment_provider.to_string(), "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
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
                flag_captured_tender_for_reversal(
                    pool,
                    &tender,
                    "cart could not be saved after capture",
                )
                .await;
                metrics::counter!(apex_edge_metrics::PAYMENT_ATTEMPTS_TOTAL, "provider" => payment_provider.to_string(), "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
                return PosResponseEnvelope {
                    version: ContractVersion::V1_0_0,
                    success: false,
                    idempotency_key,
                    payload: None,
                    errors,
                };
            }
            metrics::counter!(apex_edge_metrics::PAYMENT_ATTEMPTS_TOTAL, "provider" => payment_provider.to_string(), "outcome" => tender.metric_outcome).increment(1);
            metrics::histogram!(apex_edge_metrics::PAYMENT_DURATION_SECONDS, "provider" => payment_provider.to_string()).record(payment_started_at.elapsed().as_secs_f64());
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
            let voided_item_ids: Vec<Uuid> = cart.lines.iter().map(|l| l.item_id).collect();
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
            // Release any held stock back to availability and notify registers.
            let _ = release_cart_reservations(pool, store_id, p.cart_id).await;
            broadcast_stock_changed(app, store_id, &voided_item_ids).await;
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
            // Atomic claim: only one register can win a concurrent recall (safe handoff).
            let data = match claim_parked_cart(pool, p.parked_cart_id, register_id).await {
                Ok(ClaimOutcome::Claimed {
                    cart_data,
                    parked_by_register,
                }) => {
                    metrics::counter!(apex_edge_metrics::CART_HANDOFF_TOTAL, "outcome" => "claimed").increment(1);
                    stream_broadcast(
                        app,
                        store_id,
                        StreamKind::CartHandoff,
                        serde_json::json!({
                            "parked_cart_id": p.parked_cart_id.to_string(),
                            "claimed_by_register": register_id.to_string(),
                            "parked_by_register": parked_by_register.to_string(),
                        }),
                    )
                    .await;
                    cart_data
                }
                Ok(ClaimOutcome::AlreadyClaimed) => {
                    metrics::counter!(apex_edge_metrics::CART_HANDOFF_TOTAL, "outcome" => "conflict").increment(1);
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "CART_ALREADY_RECALLED".into(),
                            message: "Parked cart was already recalled by another register".into(),
                            field: None,
                        }],
                    };
                }
                Ok(ClaimOutcome::NotFound) => {
                    metrics::counter!(apex_edge_metrics::CART_HANDOFF_TOTAL, "outcome" => "not_found").increment(1);
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
                }
                Err(e) => {
                    metrics::counter!(apex_edge_metrics::CART_HANDOFF_TOTAL, "outcome" => "error")
                        .increment(1);
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "RECALL_FAILED".into(),
                            message: e.to_string(),
                            field: None,
                        }],
                    };
                }
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
                    // Make the local stock change immediately sellable on the edge.
                    let _ = apply_local_delta(pool, store_id, p.item_id, p.quantity_delta).await;
                    broadcast_stock_changed(app, store_id, &[p.item_id]).await;
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
            let shift_id = fetch_open_shift(pool, store_id, register_id)
                .await
                .ok()
                .flatten()
                .map(|shift| shift.id);
            let fiscal_transaction = crate::fiscal::sale_transaction(
                pool,
                &order,
                store_id,
                register_id,
                shift_id,
                &app.fiscal.currency,
            )
            .await;
            let fiscal_signing =
                crate::fiscal::sign_or_queue(app, "order", order_id, &fiscal_transaction).await;
            let fiscal_fields = match fiscal_signing {
                Ok(outcome) => {
                    crate::fiscal::fields_from_outcome(app.fiscal.provider.provider_code(), outcome)
                }
                Err(e) => {
                    // Fail closed before any ledger/outbox/stock mutation: a regulated
                    // deployment (e.g. DE-TSE) that is misconfigured must not complete a sale
                    // without a fiscal receipt. The customer's card was already captured,
                    // so hand it to the reversal sweeper rather than keeping the money.
                    flag_cart_payments_for_reversal(
                        pool,
                        store_id,
                        cart.id,
                        "fiscal signing failed",
                    )
                    .await;
                    return PosResponseEnvelope {
                        version: ContractVersion::V1_0_0,
                        success: false,
                        idempotency_key,
                        payload: None,
                        errors: vec![PosError {
                            code: "FISCAL_SIGNING_FAILED".into(),
                            message: e.to_string(),
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
                fiscal_provider: fiscal_fields.provider,
                fiscal_id: fiscal_fields.fiscal_id,
                fiscal_signature: fiscal_fields.signature,
                fiscal_qr_payload: fiscal_fields.qr_payload,
                fiscal_signed_at: fiscal_fields.signed_at,
                fiscal_pending: fiscal_fields.pending,
            };
            let took_cash = ledger_entry
                .payments
                .iter()
                .any(|payment| payment.tender_type.eq_ignore_ascii_case("cash"));
            let ledger_started_at = Instant::now();
            if let Err(e) = insert_order_ledger_entry(pool, &ledger_entry).await {
                metrics::counter!(apex_edge_metrics::ORDERS_FINALIZED_TOTAL, "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
                flag_cart_payments_for_reversal(
                    pool,
                    store_id,
                    cart.id,
                    "order ledger write failed",
                )
                .await;
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
            // The order is durable, so the money is earned. Past this point the sweeper
            // must never touch these payments, even if a later best-effort step fails.
            if let Err(e) = settle_payment_intents_for_cart(pool, store_id, cart.id, order_id).await
            {
                tracing::error!(order_id = %order_id, error = %e, "could not settle payment intents");
            }
            metrics::counter!(apex_edge_metrics::ORDERS_FINALIZED_TOTAL, "outcome" => apex_edge_metrics::OUTCOME_SUCCESS).increment(1);
            metrics::histogram!(apex_edge_metrics::ORDERS_LEDGER_WRITE_DURATION_SECONDS)
                .record(ledger_started_at.elapsed().as_secs_f64());
            // Auto-earn loyalty points for carts with an attached customer. Best-effort:
            // unlike fiscal signing, a loyalty storage hiccup must never fail an
            // otherwise-successful, already-persisted sale.
            if let Some(customer_id) = cart.customer_id {
                if order.total_cents > 0 {
                    let loyalty_started_at = Instant::now();
                    let mut earn_account = LoyaltyAccount {
                        customer_id,
                        points: 0,
                    };
                    let earn_outcome = loyalty_provider().earn(
                        &mut earn_account,
                        EarnRequest {
                            customer_id,
                            spend_cents: order.total_cents,
                        },
                    );
                    let outcome_label = match earn_outcome {
                        Ok(earned) if earned > 0 => {
                            match earn_loyalty_points(pool, customer_id, earned).await {
                                Ok(_) => apex_edge_metrics::OUTCOME_SUCCESS,
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        customer_id = %customer_id,
                                        "loyalty auto-earn failed to persist"
                                    );
                                    apex_edge_metrics::OUTCOME_ERROR
                                }
                            }
                        }
                        Ok(_) => apex_edge_metrics::OUTCOME_SUCCESS,
                        Err(_) => apex_edge_metrics::OUTCOME_ERROR,
                    };
                    metrics::counter!(apex_edge_metrics::LOYALTY_OPERATIONS_TOTAL, "operation" => "earn_auto", "outcome" => outcome_label).increment(1);
                    metrics::histogram!(apex_edge_metrics::LOYALTY_OPERATION_DURATION_SECONDS, "operation" => "earn_auto").record(loyalty_started_at.elapsed().as_secs_f64());
                }
            }
            // Commit reserved stock as sold so availability reflects the completed sale.
            let _ = commit_cart_sale(pool, store_id, cart.id).await;
            let sold_item_ids: Vec<Uuid> = order.lines.iter().map(|l| l.item_id).collect();
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
                    "tender_type": payment_tender_type(&payment.external_reference),
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
            broadcast_stock_changed(app, store_id, &sold_item_ids).await;

            // Past this point the sale is complete and irreversible. A printer that is
            // out of paper or unplugged is reported, never allowed to undo a sale.
            let print_error = print_receipt_after_finalize(app, &receipt_payload, took_cash);

            let result = FinalizeResult {
                order_id,
                cart_id: cart.id,
                total_cents: order.total_cents,
                print_job_ids: vec![doc_id],
                print_error,
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
            let removed_item_id = cart
                .lines
                .iter()
                .find(|l| l.line_id == p.line_id)
                .map(|l| l.item_id);
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
            // Return the removed line's held stock to availability and notify registers.
            let _ = release_line_reservation(pool, store_id, p.line_id).await;
            if let Some(item_id) = removed_item_id {
                broadcast_stock_changed(app, store_id, &[item_id]).await;
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
        PosCommand::IssueGiftCard(p) => {
            let op_started_at = Instant::now();
            let code = p.code.clone().unwrap_or_else(generate_gift_card_code);
            let (outcome_label, result) =
                match issue_gift_card(pool, store_id, &code, &p.currency).await {
                    Ok((IssueOutcome::Issued, record)) => (
                        apex_edge_metrics::OUTCOME_SUCCESS,
                        PosResponseEnvelope {
                            version: ContractVersion::V1_0_0,
                            success: true,
                            idempotency_key,
                            payload: record.as_ref().map(gift_card_info_payload),
                            errors: vec![],
                        },
                    ),
                    Ok((IssueOutcome::DuplicateCode, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "GIFT_CARD_CODE_EXISTS",
                            "Gift card code already exists",
                        ),
                    ),
                    Err(e) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(idempotency_key, "GIFT_CARD_ISSUE_FAILED", e.to_string()),
                    ),
                };
            metrics::counter!(apex_edge_metrics::GIFT_CARD_OPERATIONS_TOTAL, "operation" => "issue", "outcome" => outcome_label).increment(1);
            metrics::histogram!(apex_edge_metrics::GIFT_CARD_OPERATION_DURATION_SECONDS, "operation" => "issue").record(op_started_at.elapsed().as_secs_f64());
            result
        }
        PosCommand::ActivateGiftCard(p) => {
            let op_started_at = Instant::now();
            let (outcome_label, result) =
                match activate_gift_card(pool, &p.code, p.opening_balance_cents).await {
                    Ok((ActivateOutcome::Activated, record)) => (
                        apex_edge_metrics::OUTCOME_SUCCESS,
                        PosResponseEnvelope {
                            version: ContractVersion::V1_0_0,
                            success: true,
                            idempotency_key,
                            payload: record.as_ref().map(gift_card_info_payload),
                            errors: vec![],
                        },
                    ),
                    Ok((ActivateOutcome::NotFound, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "GIFT_CARD_NOT_FOUND",
                            "Gift card not found",
                        ),
                    ),
                    Ok((ActivateOutcome::AlreadyActivated, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "GIFT_CARD_ALREADY_ACTIVE",
                            "Gift card has already been activated",
                        ),
                    ),
                    Ok((ActivateOutcome::InvalidAmount, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "INVALID_AMOUNT",
                            "Opening balance must be greater than zero",
                        ),
                    ),
                    Err(e) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "GIFT_CARD_ACTIVATE_FAILED",
                            e.to_string(),
                        ),
                    ),
                };
            metrics::counter!(apex_edge_metrics::GIFT_CARD_OPERATIONS_TOTAL, "operation" => "activate", "outcome" => outcome_label).increment(1);
            metrics::histogram!(apex_edge_metrics::GIFT_CARD_OPERATION_DURATION_SECONDS, "operation" => "activate").record(op_started_at.elapsed().as_secs_f64());
            result
        }
        PosCommand::ReloadGiftCard(p) => {
            let op_started_at = Instant::now();
            let (outcome_label, result) =
                match reload_gift_card(pool, &p.code, p.amount_cents).await {
                    Ok((ReloadOutcome::Reloaded, record)) => (
                        apex_edge_metrics::OUTCOME_SUCCESS,
                        PosResponseEnvelope {
                            version: ContractVersion::V1_0_0,
                            success: true,
                            idempotency_key,
                            payload: record.as_ref().map(gift_card_info_payload),
                            errors: vec![],
                        },
                    ),
                    Ok((ReloadOutcome::NotFound, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "GIFT_CARD_NOT_FOUND",
                            "Gift card not found",
                        ),
                    ),
                    Ok((ReloadOutcome::NotActive, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "GIFT_CARD_NOT_ACTIVE",
                            "Gift card is not active",
                        ),
                    ),
                    Ok((ReloadOutcome::InvalidAmount, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "INVALID_AMOUNT",
                            "Reload amount must be greater than zero",
                        ),
                    ),
                    Err(e) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(idempotency_key, "GIFT_CARD_RELOAD_FAILED", e.to_string()),
                    ),
                };
            metrics::counter!(apex_edge_metrics::GIFT_CARD_OPERATIONS_TOTAL, "operation" => "reload", "outcome" => outcome_label).increment(1);
            metrics::histogram!(apex_edge_metrics::GIFT_CARD_OPERATION_DURATION_SECONDS, "operation" => "reload").record(op_started_at.elapsed().as_secs_f64());
            result
        }
        PosCommand::RedeemGiftCard(p) => {
            let op_started_at = Instant::now();
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                metrics::counter!(apex_edge_metrics::GIFT_CARD_OPERATIONS_TOTAL, "operation" => "redeem", "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
                return gift_card_error(idempotency_key, "CART_NOT_FOUND", "Cart not found");
            };
            // Validate the cart can accept a payment *before* debiting the card, so a bad
            // cart state never costs the customer money without recording a payment.
            if cart.state != CartStateKind::Tendering && cart.state != CartStateKind::Paid {
                metrics::counter!(apex_edge_metrics::GIFT_CARD_OPERATIONS_TOTAL, "operation" => "redeem", "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
                return gift_card_error(
                    idempotency_key,
                    "INVALID_PAYMENT",
                    "Cannot add payment in current state",
                );
            }
            let (outcome_label, result) =
                match redeem_gift_card(pool, &p.code, p.amount_cents).await {
                    Ok((RedeemOutcome::Redeemed, _record)) => {
                        let add_payment_result = cart.add_payment(AddPaymentInput {
                            tender_id: p.tender_id,
                            amount_cents: p.amount_cents,
                            tip_amount_cents: 0,
                            external_reference: Some(format!("gift_card:{}", p.code)),
                            provider: Some("gift_card".into()),
                            provider_payment_id: Some(p.code.clone()),
                            entry_method: None,
                        });
                        if add_payment_result.is_err() {
                            (
                                apex_edge_metrics::OUTCOME_ERROR,
                                gift_card_error(
                                    idempotency_key,
                                    "INVALID_PAYMENT",
                                    "Cannot add payment in current state",
                                ),
                            )
                        } else if let Err(errors) = save_cart_to_db(pool, &cart).await {
                            (
                                apex_edge_metrics::OUTCOME_ERROR,
                                PosResponseEnvelope {
                                    version: ContractVersion::V1_0_0,
                                    success: false,
                                    idempotency_key,
                                    payload: None,
                                    errors,
                                },
                            )
                        } else {
                            let state = build_cart_state(pool, store_id, &cart).await;
                            (
                                apex_edge_metrics::OUTCOME_SUCCESS,
                                PosResponseEnvelope {
                                    version: ContractVersion::V1_0_0,
                                    success: true,
                                    idempotency_key,
                                    payload: Some(cart_state_to_payload(&state)),
                                    errors: vec![],
                                },
                            )
                        }
                    }
                    Ok((RedeemOutcome::NotFound, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "GIFT_CARD_NOT_FOUND",
                            "Gift card not found",
                        ),
                    ),
                    Ok((RedeemOutcome::NotActive, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "GIFT_CARD_NOT_ACTIVE",
                            "Gift card is not active",
                        ),
                    ),
                    Ok((RedeemOutcome::InsufficientBalance, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "INSUFFICIENT_GIFT_CARD_BALANCE",
                            "Gift card balance is insufficient",
                        ),
                    ),
                    Ok((RedeemOutcome::InvalidAmount, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(
                            idempotency_key,
                            "INVALID_AMOUNT",
                            "Redeem amount must be greater than zero",
                        ),
                    ),
                    Err(e) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        gift_card_error(idempotency_key, "GIFT_CARD_REDEEM_FAILED", e.to_string()),
                    ),
                };
            metrics::counter!(apex_edge_metrics::GIFT_CARD_OPERATIONS_TOTAL, "operation" => "redeem", "outcome" => outcome_label).increment(1);
            metrics::histogram!(apex_edge_metrics::GIFT_CARD_OPERATION_DURATION_SECONDS, "operation" => "redeem").record(op_started_at.elapsed().as_secs_f64());
            result
        }
        PosCommand::EarnLoyaltyPoints(p) => {
            let op_started_at = Instant::now();
            let (outcome_label, result) = if p.spend_cents == 0 {
                (
                    apex_edge_metrics::OUTCOME_ERROR,
                    loyalty_error(
                        idempotency_key,
                        "INVALID_AMOUNT",
                        "Spend amount must be greater than zero",
                    ),
                )
            } else {
                let mut earn_account = LoyaltyAccount {
                    customer_id: p.customer_id,
                    points: 0,
                };
                match loyalty_provider().earn(
                    &mut earn_account,
                    EarnRequest {
                        customer_id: p.customer_id,
                        spend_cents: p.spend_cents,
                    },
                ) {
                    Ok(earned) => match earn_loyalty_points(pool, p.customer_id, earned).await {
                        Ok(record) => (
                            apex_edge_metrics::OUTCOME_SUCCESS,
                            PosResponseEnvelope {
                                version: ContractVersion::V1_0_0,
                                success: true,
                                idempotency_key,
                                payload: Some(loyalty_info_payload(&record)),
                                errors: vec![],
                            },
                        ),
                        Err(e) => (
                            apex_edge_metrics::OUTCOME_ERROR,
                            loyalty_error(idempotency_key, "LOYALTY_EARN_FAILED", e.to_string()),
                        ),
                    },
                    Err(e) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        loyalty_error(idempotency_key, "INVALID_AMOUNT", e.to_string()),
                    ),
                }
            };
            metrics::counter!(apex_edge_metrics::LOYALTY_OPERATIONS_TOTAL, "operation" => "earn", "outcome" => outcome_label).increment(1);
            metrics::histogram!(apex_edge_metrics::LOYALTY_OPERATION_DURATION_SECONDS, "operation" => "earn").record(op_started_at.elapsed().as_secs_f64());
            result
        }
        PosCommand::RedeemLoyaltyPoints(p) => {
            let op_started_at = Instant::now();
            let Some(mut cart) = load_cart_from_db(pool, p.cart_id).await.ok().flatten() else {
                metrics::counter!(apex_edge_metrics::LOYALTY_OPERATIONS_TOTAL, "operation" => "redeem", "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
                return loyalty_error(idempotency_key, "CART_NOT_FOUND", "Cart not found");
            };
            // Validate the cart can accept a payment *before* debiting points, so a bad
            // cart state never costs the customer points without recording a payment.
            if cart.state != CartStateKind::Tendering && cart.state != CartStateKind::Paid {
                metrics::counter!(apex_edge_metrics::LOYALTY_OPERATIONS_TOTAL, "operation" => "redeem", "outcome" => apex_edge_metrics::OUTCOME_ERROR).increment(1);
                return loyalty_error(
                    idempotency_key,
                    "INVALID_PAYMENT",
                    "Cannot add payment in current state",
                );
            }
            let (outcome_label, result) =
                match redeem_loyalty_points(pool, p.customer_id, p.points).await {
                    Ok((RedeemPointsOutcome::Redeemed, _record)) => {
                        let value_cents = loyalty_redeem_value_cents(p.customer_id, p.points);
                        let add_payment_result = cart.add_payment(AddPaymentInput {
                            tender_id: p.tender_id,
                            amount_cents: value_cents,
                            tip_amount_cents: 0,
                            external_reference: Some(format!("loyalty:{}", p.customer_id)),
                            provider: Some("loyalty".into()),
                            provider_payment_id: Some(p.points.to_string()),
                            entry_method: None,
                        });
                        if add_payment_result.is_err() {
                            (
                                apex_edge_metrics::OUTCOME_ERROR,
                                loyalty_error(
                                    idempotency_key,
                                    "INVALID_PAYMENT",
                                    "Cannot add payment in current state",
                                ),
                            )
                        } else if let Err(errors) = save_cart_to_db(pool, &cart).await {
                            (
                                apex_edge_metrics::OUTCOME_ERROR,
                                PosResponseEnvelope {
                                    version: ContractVersion::V1_0_0,
                                    success: false,
                                    idempotency_key,
                                    payload: None,
                                    errors,
                                },
                            )
                        } else {
                            let state = build_cart_state(pool, store_id, &cart).await;
                            (
                                apex_edge_metrics::OUTCOME_SUCCESS,
                                PosResponseEnvelope {
                                    version: ContractVersion::V1_0_0,
                                    success: true,
                                    idempotency_key,
                                    payload: Some(cart_state_to_payload(&state)),
                                    errors: vec![],
                                },
                            )
                        }
                    }
                    Ok((RedeemPointsOutcome::NotFound, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        loyalty_error(
                            idempotency_key,
                            "LOYALTY_ACCOUNT_NOT_FOUND",
                            "Loyalty account not found",
                        ),
                    ),
                    Ok((RedeemPointsOutcome::InsufficientPoints, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        loyalty_error(
                            idempotency_key,
                            "INSUFFICIENT_LOYALTY_POINTS",
                            "Loyalty points balance is insufficient",
                        ),
                    ),
                    Ok((RedeemPointsOutcome::InvalidAmount, _)) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        loyalty_error(
                            idempotency_key,
                            "INVALID_AMOUNT",
                            "Redeem points must be greater than zero",
                        ),
                    ),
                    Err(e) => (
                        apex_edge_metrics::OUTCOME_ERROR,
                        loyalty_error(idempotency_key, "LOYALTY_REDEEM_FAILED", e.to_string()),
                    ),
                };
            metrics::counter!(apex_edge_metrics::LOYALTY_OPERATIONS_TOTAL, "operation" => "redeem", "outcome" => outcome_label).increment(1);
            metrics::histogram!(apex_edge_metrics::LOYALTY_OPERATION_DURATION_SECONDS, "operation" => "redeem").record(op_started_at.elapsed().as_secs_f64());
            result
        }
        PosCommand::PrintDocument(p) => print_document_command(app, idempotency_key, p).await,
    };
    result
}

/// Prints the receipt for a just-finalized sale and opens the drawer if policy says so.
///
/// Returns the message to hand back to the operator, or `None` when there was nothing to
/// report — either it printed, or this hub has no printer, which is the normal case.
fn print_receipt_after_finalize(
    app: &AppState,
    receipt_payload: &serde_json::Value,
    took_cash: bool,
) -> Option<String> {
    let print_error = match app.hardware.print_receipt(receipt_payload) {
        Ok(_) => None,
        Err(e) => Some(e.to_string()),
    };
    // The drawer is attached to the printer, so a failed receipt does not mean a failed
    // kick: cash still has to go somewhere.
    if app.hardware.should_kick_drawer(took_cash) {
        if let Err(e) = app.hardware.kick_drawer() {
            tracing::error!(error = %e, "cash drawer did not open");
            return Some(print_error.map_or_else(
                || e.to_string(),
                |printed| format!("{printed}; cash drawer: {e}"),
            ));
        }
    }
    print_error
}

/// Reprints a document the hub has already generated.
async fn print_document_command(
    app: &AppState,
    idempotency_key: Uuid,
    payload: &apex_edge_contracts::PrintDocumentPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let print_error = |code: &str, message: String| PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: false,
        idempotency_key,
        payload: None,
        errors: vec![PosError {
            code: code.into(),
            message,
            field: None,
        }],
    };

    if app.hardware.device.is_none() {
        // Silently succeeding would let an operator stand at a printer that will never
        // produce paper, waiting.
        return print_error(
            "PRINTER_NOT_CONFIGURED",
            "This hub has no printer; fetch the document and print it from the POS".into(),
        );
    }

    let document = match apex_edge_storage::get_document(&app.pool, payload.document_id).await {
        Ok(Some(document)) => document,
        Ok(None) => {
            return print_error("DOCUMENT_NOT_FOUND", "Document not found".into());
        }
        Err(e) => {
            return print_error("DOCUMENT_LOOKUP_FAILED", e.to_string());
        }
    };

    let receipt_payload = serde_json::from_str::<serde_json::Value>(&document.payload)
        .unwrap_or(serde_json::Value::Null);
    let encoder = match app.hardware.print_receipt(&receipt_payload) {
        Ok(encoder) => encoder,
        Err(e) => return print_error("PRINT_FAILED", e.to_string()),
    };

    let drawer_opened = if payload.open_drawer {
        match app.hardware.kick_drawer() {
            Ok(opened) => opened,
            Err(e) => return print_error("DRAWER_FAILED", e.to_string()),
        }
    } else {
        false
    };

    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(serde_json::json!({
            "document_id": payload.document_id.to_string(),
            "document_type": document.document_type,
            "encoder": encoder,
            "drawer_opened": drawer_opened,
        })),
        errors: vec![],
    }
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
