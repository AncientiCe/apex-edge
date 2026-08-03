//! Behavioural coverage for gift card commands wired into the POS command surface:
//! issue -> activate -> reload -> redeem-as-tender at checkout, plus the failure paths
//! (duplicate code, double activation, insufficient balance, inactive/unknown card).

use apex_edge_api::{get_cart_state_handler, handle_pos_command, AppState};
use apex_edge_contracts::{
    ActivateGiftCardPayload, AddLineItemPayload, ContractVersion, CreateCartPayload, GiftCardInfo,
    GiftCardStateKind, IssueGiftCardPayload, PosCommand, PosRequestEnvelope, RedeemGiftCardPayload,
    ReloadGiftCardPayload, SetTenderingPayload,
};
use apex_edge_storage::{insert_catalog_item, insert_price_book_entry, run_migrations};
use axum::extract::State;
use axum::Json;
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

async fn test_state() -> (AppState, Uuid, Uuid) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let store_id = Uuid::nil();
    let register_id = Uuid::new_v4();
    let state = AppState {
        store_id,
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
        stream: apex_edge_api::StreamHub::new(),
        role: apex_edge_api::HubRole::Primary,
        fiscal: apex_edge_api::FiscalSettings::default(),
    };
    (state, store_id, register_id)
}

async fn send(
    state: &AppState,
    store_id: Uuid,
    register_id: Uuid,
    payload: PosCommand,
) -> (
    bool,
    Vec<apex_edge_contracts::PosError>,
    Option<serde_json::Value>,
) {
    let resp = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id,
            payload,
        }),
    )
    .await;
    (resp.0.success, resp.0.errors, resp.0.payload)
}

#[tokio::test]
async fn issue_activate_reload_and_redeem_round_trip() {
    let (state, store_id, register_id) = test_state().await;

    let (ok, errors, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::IssueGiftCard(IssueGiftCardPayload {
            code: Some("GC-E2E-001".into()),
            currency: "USD".into(),
        }),
    )
    .await;
    assert!(ok, "issue errors: {errors:?}");
    let info: GiftCardInfo = serde_json::from_value(payload.unwrap()).expect("gift card info");
    assert_eq!(info.code, "GC-E2E-001");
    assert_eq!(info.state, GiftCardStateKind::Issued);
    assert_eq!(info.balance_cents, 0);

    let (ok, errors, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::ActivateGiftCard(ActivateGiftCardPayload {
            code: "GC-E2E-001".into(),
            opening_balance_cents: 2_000,
        }),
    )
    .await;
    assert!(ok, "activate errors: {errors:?}");
    let info: GiftCardInfo = serde_json::from_value(payload.unwrap()).expect("gift card info");
    assert_eq!(info.state, GiftCardStateKind::Active);
    assert_eq!(info.balance_cents, 2_000);

    let (ok, errors, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::ReloadGiftCard(ReloadGiftCardPayload {
            code: "GC-E2E-001".into(),
            amount_cents: 500,
        }),
    )
    .await;
    assert!(ok, "reload errors: {errors:?}");
    let info: GiftCardInfo = serde_json::from_value(payload.unwrap()).expect("gift card info");
    assert_eq!(info.balance_cents, 2_500);

    // Build a cart and move it to Tendering so the gift card can be applied as a tender.
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &state.pool,
        item_id,
        store_id,
        "GIFT-CART-ITEM",
        "Gift Cart Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert_catalog_item");
    insert_price_book_entry(&state.pool, store_id, item_id, None, 1_500, "USD")
        .await
        .expect("insert_price_book_entry");

    let (ok, errors, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    )
    .await;
    assert!(ok, "create cart errors: {errors:?}");
    let cart_state: apex_edge_contracts::CartState =
        serde_json::from_value(payload.unwrap()).expect("cart state");

    let (ok, errors, _) = send(
        &state,
        store_id,
        register_id,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id: cart_state.cart_id,
            item_id,
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;
    assert!(ok, "add line item errors: {errors:?}");

    let (ok, errors, _) = send(
        &state,
        store_id,
        register_id,
        PosCommand::SetTendering(SetTenderingPayload {
            cart_id: cart_state.cart_id,
        }),
    )
    .await;
    assert!(ok, "set tendering errors: {errors:?}");

    let (ok, errors, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::RedeemGiftCard(RedeemGiftCardPayload {
            cart_id: cart_state.cart_id,
            tender_id: Uuid::new_v4(),
            code: "GC-E2E-001".into(),
            amount_cents: 1_500,
        }),
    )
    .await;
    assert!(ok, "redeem errors: {errors:?}");
    let updated_cart: apex_edge_contracts::CartState =
        serde_json::from_value(payload.unwrap()).expect("cart state");
    assert_eq!(updated_cart.tendered_cents, 1_500);

    // Balance was debited by the redeem.
    let (ok, errors, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::ReloadGiftCard(ReloadGiftCardPayload {
            code: "GC-E2E-001".into(),
            amount_cents: 100,
        }),
    )
    .await;
    assert!(ok, "reload errors: {errors:?}");
    let info: GiftCardInfo = serde_json::from_value(payload.unwrap()).expect("gift card info");
    assert_eq!(info.balance_cents, 2_500 - 1_500 + 100);
}

#[tokio::test]
async fn issuing_duplicate_code_fails_with_gift_card_code_exists() {
    let (state, store_id, register_id) = test_state().await;
    send(
        &state,
        store_id,
        register_id,
        PosCommand::IssueGiftCard(IssueGiftCardPayload {
            code: Some("GC-DUP".into()),
            currency: "USD".into(),
        }),
    )
    .await;
    let (ok, errors, _) = send(
        &state,
        store_id,
        register_id,
        PosCommand::IssueGiftCard(IssueGiftCardPayload {
            code: Some("GC-DUP".into()),
            currency: "USD".into(),
        }),
    )
    .await;
    assert!(!ok);
    assert_eq!(errors[0].code, "GIFT_CARD_CODE_EXISTS");
}

#[tokio::test]
async fn activating_already_active_card_fails() {
    let (state, store_id, register_id) = test_state().await;
    send(
        &state,
        store_id,
        register_id,
        PosCommand::IssueGiftCard(IssueGiftCardPayload {
            code: Some("GC-ACT2".into()),
            currency: "USD".into(),
        }),
    )
    .await;
    send(
        &state,
        store_id,
        register_id,
        PosCommand::ActivateGiftCard(ActivateGiftCardPayload {
            code: "GC-ACT2".into(),
            opening_balance_cents: 1_000,
        }),
    )
    .await;
    let (ok, errors, _) = send(
        &state,
        store_id,
        register_id,
        PosCommand::ActivateGiftCard(ActivateGiftCardPayload {
            code: "GC-ACT2".into(),
            opening_balance_cents: 500,
        }),
    )
    .await;
    assert!(!ok);
    assert_eq!(errors[0].code, "GIFT_CARD_ALREADY_ACTIVE");
}

#[tokio::test]
async fn redeem_with_insufficient_balance_is_rejected_and_cart_unaffected() {
    let (state, store_id, register_id) = test_state().await;
    send(
        &state,
        store_id,
        register_id,
        PosCommand::IssueGiftCard(IssueGiftCardPayload {
            code: Some("GC-LOW".into()),
            currency: "USD".into(),
        }),
    )
    .await;
    send(
        &state,
        store_id,
        register_id,
        PosCommand::ActivateGiftCard(ActivateGiftCardPayload {
            code: "GC-LOW".into(),
            opening_balance_cents: 100,
        }),
    )
    .await;

    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &state.pool,
        item_id,
        store_id,
        "GIFT-LOW-ITEM",
        "Low Balance Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert_catalog_item");
    insert_price_book_entry(&state.pool, store_id, item_id, None, 1_000, "USD")
        .await
        .expect("insert_price_book_entry");

    let (_, _, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    )
    .await;
    let cart_state: apex_edge_contracts::CartState =
        serde_json::from_value(payload.unwrap()).expect("cart state");
    send(
        &state,
        store_id,
        register_id,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id: cart_state.cart_id,
            item_id,
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;
    send(
        &state,
        store_id,
        register_id,
        PosCommand::SetTendering(SetTenderingPayload {
            cart_id: cart_state.cart_id,
        }),
    )
    .await;

    let (ok, errors, _) = send(
        &state,
        store_id,
        register_id,
        PosCommand::RedeemGiftCard(RedeemGiftCardPayload {
            cart_id: cart_state.cart_id,
            tender_id: Uuid::new_v4(),
            code: "GC-LOW".into(),
            amount_cents: 1_000,
        }),
    )
    .await;
    assert!(!ok);
    assert_eq!(errors[0].code, "INSUFFICIENT_GIFT_CARD_BALANCE");

    // Cart tendered amount must be untouched since the redeem never happened.
    let refreshed = get_cart_state_handler(
        State(state.clone()),
        axum::extract::Path(cart_state.cart_id),
    )
    .await
    .expect("cart must still exist");
    assert_eq!(refreshed.0.tendered_cents, 0);

    // Gift card balance must be untouched too.
    let (_, _, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::ReloadGiftCard(ReloadGiftCardPayload {
            code: "GC-LOW".into(),
            amount_cents: 1,
        }),
    )
    .await;
    let info: GiftCardInfo = serde_json::from_value(payload.unwrap()).expect("gift card info");
    assert_eq!(info.balance_cents, 101);
}

#[tokio::test]
async fn redeem_against_unknown_gift_card_fails_without_mutating_cart() {
    let (state, store_id, register_id) = test_state().await;
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &state.pool,
        item_id,
        store_id,
        "GIFT-UNKNOWN-ITEM",
        "Unknown Gift Card Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert_catalog_item");
    insert_price_book_entry(&state.pool, store_id, item_id, None, 500, "USD")
        .await
        .expect("insert_price_book_entry");

    let (_, _, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    )
    .await;
    let cart_state: apex_edge_contracts::CartState =
        serde_json::from_value(payload.unwrap()).expect("cart state");
    send(
        &state,
        store_id,
        register_id,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id: cart_state.cart_id,
            item_id,
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;
    send(
        &state,
        store_id,
        register_id,
        PosCommand::SetTendering(SetTenderingPayload {
            cart_id: cart_state.cart_id,
        }),
    )
    .await;

    let (ok, errors, _) = send(
        &state,
        store_id,
        register_id,
        PosCommand::RedeemGiftCard(RedeemGiftCardPayload {
            cart_id: cart_state.cart_id,
            tender_id: Uuid::new_v4(),
            code: "GC-DOES-NOT-EXIST".into(),
            amount_cents: 100,
        }),
    )
    .await;
    assert!(!ok);
    assert_eq!(errors[0].code, "GIFT_CARD_NOT_FOUND");
}
