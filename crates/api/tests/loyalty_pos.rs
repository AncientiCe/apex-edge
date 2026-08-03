//! Behavioural coverage for loyalty points: manual earn/redeem commands, auto-earn on
//! `FinalizeOrder` for carts with an attached customer, and redemption as a cart tender.

use apex_edge_api::{get_cart_state_handler, handle_pos_command, AppState};
use apex_edge_contracts::{
    AddLineItemPayload, ContractVersion, CreateCartPayload, EarnLoyaltyPointsPayload,
    FinalizeOrderPayload, FinalizeResult, LoyaltyAccountInfo, PosCommand, PosRequestEnvelope,
    RedeemLoyaltyPointsPayload, SetCustomerPayload, SetTenderingPayload,
};
use apex_edge_storage::{
    insert_catalog_item, insert_customer, insert_price_book_entry, run_migrations,
};
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
async fn manual_earn_then_redeem_round_trip() {
    let (state, store_id, register_id) = test_state().await;
    let customer_id = Uuid::new_v4();
    insert_customer(
        &state.pool,
        customer_id,
        store_id,
        "CUST-1",
        "Jane Doe",
        None,
    )
    .await
    .expect("insert_customer");

    // Default rate: 100 cents per point -> 1500 cents earns 15 points.
    let (ok, errors, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::EarnLoyaltyPoints(EarnLoyaltyPointsPayload {
            customer_id,
            spend_cents: 1_500,
        }),
    )
    .await;
    assert!(ok, "earn errors: {errors:?}");
    let info: LoyaltyAccountInfo = serde_json::from_value(payload.unwrap()).expect("loyalty info");
    assert_eq!(info.points, 15);

    // Build a cart to redeem the points against as a tender.
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &state.pool,
        item_id,
        store_id,
        "LOYALTY-ITEM",
        "Loyalty Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert_catalog_item");
    // Default redeem rate: 1 point = 1 cent, so 10 points = 10 cents; price the item at 10c.
    insert_price_book_entry(&state.pool, store_id, item_id, None, 10, "USD")
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

    let (ok, errors, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::RedeemLoyaltyPoints(RedeemLoyaltyPointsPayload {
            cart_id: cart_state.cart_id,
            tender_id: Uuid::new_v4(),
            customer_id,
            points: 10,
        }),
    )
    .await;
    assert!(ok, "redeem errors: {errors:?}");
    let updated_cart: apex_edge_contracts::CartState =
        serde_json::from_value(payload.unwrap()).expect("cart state");
    assert_eq!(updated_cart.tendered_cents, 10);

    // Remaining balance: 15 - 10 = 5 points.
    let account = apex_edge_storage::get_loyalty_account(&state.pool, customer_id)
        .await
        .expect("query loyalty account")
        .expect("account exists");
    assert_eq!(account.points, 5);
}

#[tokio::test]
async fn earning_zero_spend_is_rejected() {
    let (state, store_id, register_id) = test_state().await;
    let (ok, errors, _) = send(
        &state,
        store_id,
        register_id,
        PosCommand::EarnLoyaltyPoints(EarnLoyaltyPointsPayload {
            customer_id: Uuid::new_v4(),
            spend_cents: 0,
        }),
    )
    .await;
    assert!(!ok);
    assert_eq!(errors[0].code, "INVALID_AMOUNT");
}

#[tokio::test]
async fn redeem_with_insufficient_points_is_rejected_and_cart_unaffected() {
    let (state, store_id, register_id) = test_state().await;
    let customer_id = Uuid::new_v4();
    insert_customer(
        &state.pool,
        customer_id,
        store_id,
        "CUST-2",
        "Low Points",
        None,
    )
    .await
    .expect("insert_customer");
    send(
        &state,
        store_id,
        register_id,
        PosCommand::EarnLoyaltyPoints(EarnLoyaltyPointsPayload {
            customer_id,
            spend_cents: 100, // 1 point
        }),
    )
    .await;

    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &state.pool,
        item_id,
        store_id,
        "LOYALTY-LOW-ITEM",
        "Loyalty Low Points Item",
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
        PosCommand::RedeemLoyaltyPoints(RedeemLoyaltyPointsPayload {
            cart_id: cart_state.cart_id,
            tender_id: Uuid::new_v4(),
            customer_id,
            points: 500,
        }),
    )
    .await;
    assert!(!ok);
    assert_eq!(errors[0].code, "INSUFFICIENT_LOYALTY_POINTS");

    let refreshed = get_cart_state_handler(
        State(state.clone()),
        axum::extract::Path(cart_state.cart_id),
    )
    .await
    .expect("cart must still exist");
    assert_eq!(refreshed.0.tendered_cents, 0);
}

#[tokio::test]
async fn finalize_order_auto_earns_points_for_attached_customer() {
    let (state, store_id, register_id) = test_state().await;
    let customer_id = Uuid::new_v4();
    insert_customer(
        &state.pool,
        customer_id,
        store_id,
        "CUST-3",
        "Auto Earn",
        None,
    )
    .await
    .expect("insert_customer");

    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &state.pool,
        item_id,
        store_id,
        "AUTO-EARN-ITEM",
        "Auto Earn Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert_catalog_item");
    // 2_000 cents total / 100 cents-per-point = 20 points earned.
    insert_price_book_entry(&state.pool, store_id, item_id, None, 2_000, "USD")
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
        PosCommand::SetCustomer(SetCustomerPayload {
            cart_id: cart_state.cart_id,
            customer_id,
        }),
    )
    .await;
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
    send(
        &state,
        store_id,
        register_id,
        PosCommand::AddPayment(apex_edge_contracts::AddPaymentPayload {
            cart_id: cart_state.cart_id,
            tender_id: Uuid::new_v4(),
            amount_cents: 2_000,
            tip_amount_cents: 0,
            external_reference: Some("cash".into()),
            provider: None,
            provider_payment_id: None,
            entry_method: None,
        }),
    )
    .await;

    let (ok, errors, payload) = send(
        &state,
        store_id,
        register_id,
        PosCommand::FinalizeOrder(FinalizeOrderPayload {
            cart_id: cart_state.cart_id,
        }),
    )
    .await;
    assert!(ok, "finalize errors: {errors:?}");
    let _result: FinalizeResult =
        serde_json::from_value(payload.unwrap()).expect("finalize payload");

    let account = apex_edge_storage::get_loyalty_account(&state.pool, customer_id)
        .await
        .expect("query loyalty account")
        .expect("account created by auto-earn");
    assert_eq!(account.points, 20);
}

#[tokio::test]
async fn finalize_order_without_customer_does_not_create_loyalty_account() {
    let (state, store_id, register_id) = test_state().await;
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &state.pool,
        item_id,
        store_id,
        "NO-CUSTOMER-ITEM",
        "No Customer Test Item",
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
    send(
        &state,
        store_id,
        register_id,
        PosCommand::AddPayment(apex_edge_contracts::AddPaymentPayload {
            cart_id: cart_state.cart_id,
            tender_id: Uuid::new_v4(),
            amount_cents: 500,
            tip_amount_cents: 0,
            external_reference: Some("cash".into()),
            provider: None,
            provider_payment_id: None,
            entry_method: None,
        }),
    )
    .await;
    let (ok, errors, _) = send(
        &state,
        store_id,
        register_id,
        PosCommand::FinalizeOrder(FinalizeOrderPayload {
            cart_id: cart_state.cart_id,
        }),
    )
    .await;
    assert!(ok, "finalize errors: {errors:?}");

    let account = apex_edge_storage::get_loyalty_account(&state.pool, Uuid::nil())
        .await
        .expect("query loyalty account");
    assert!(account.is_none());
}
