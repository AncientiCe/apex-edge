//! POS commands that previously had no behavioural coverage: manual discounts, listing
//! parked carts, and stock transfers/adjustments.

use apex_edge_api::{pos_handler::execute_pos_command, AppState};
use apex_edge_contracts::{
    AddLineItemPayload, ApplyManualDiscountPayload, ContractVersion, CreateCartPayload,
    ListParkedCartsPayload, ManualDiscountKind, ParkCartPayload, PosCommand, PosRequestEnvelope,
    PosResponseEnvelope, StockMovementPayload,
};
use apex_edge_storage::{
    create_sqlite_pool, get_inventory_state, insert_price_book_entry, run_migrations,
    seed_inventory_from_catalog, set_audit_key, AuditKey,
};
use serde_json::Value;
use uuid::Uuid;

const STORE: Uuid = Uuid::nil();

/// A hub with one tracked item priced at 1000 cents and `qty` units on hand.
async fn setup(qty: i64) -> (AppState, Uuid) {
    set_audit_key(AuditKey::new("test-hub", b"test-secret".to_vec()));
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let item_id = Uuid::new_v4();
    let cat = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO catalog_items (id, store_id, sku, name, category_id, tax_category_id, is_active, available_qty) \
         VALUES (?, ?, 'SKU-1', 'Widget', ?, ?, 1, ?)",
    )
    .bind(item_id.to_string())
    .bind(STORE.to_string())
    .bind(cat.to_string())
    .bind(cat.to_string())
    .bind(qty)
    .execute(&pool)
    .await
    .unwrap();
    insert_price_book_entry(&pool, STORE, item_id, None, 1000, "USD")
        .await
        .unwrap();
    seed_inventory_from_catalog(&pool, STORE).await.unwrap();
    (AppState::new(pool, STORE), item_id)
}

async fn send(state: &AppState, register: Uuid, command: PosCommand) -> PosResponseEnvelope<Value> {
    execute_pos_command(
        state,
        PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id: STORE,
            register_id: register,
            payload: command,
        },
    )
    .await
}

/// A cart holding `quantity` units of the item; returns (cart_id, line_id).
async fn cart_with(state: &AppState, register: Uuid, item_id: Uuid, quantity: u32) -> (Uuid, Uuid) {
    let cart_id = Uuid::new_v4();
    let created = send(
        state,
        register,
        PosCommand::CreateCart(CreateCartPayload {
            cart_id: Some(cart_id),
        }),
    )
    .await;
    assert!(created.success, "{:?}", created.errors);
    let added = send(
        state,
        register,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id,
            item_id,
            modifier_option_ids: vec![],
            quantity,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;
    assert!(added.success, "{:?}", added.errors);
    let line_id = added.payload.expect("cart")["lines"][0]["line_id"]
        .as_str()
        .expect("line id")
        .parse()
        .expect("uuid");
    (cart_id, line_id)
}

fn discount(
    cart_id: Uuid,
    kind: ManualDiscountKind,
    value: u64,
    line_id: Option<Uuid>,
    reason: &str,
) -> PosCommand {
    PosCommand::ApplyManualDiscount(ApplyManualDiscountPayload {
        cart_id,
        reason: reason.into(),
        kind,
        value,
        line_id,
    })
}

fn code(resp: &PosResponseEnvelope<Value>) -> &str {
    resp.errors.first().map(|e| e.code.as_str()).unwrap_or("")
}

// ---------- apply_manual_discount ----------

#[tokio::test]
async fn percent_cart_discount_takes_basis_points_of_the_subtotal() {
    let (state, item) = setup(10).await;
    let reg = Uuid::new_v4();
    let (cart, _) = cart_with(&state, reg, item, 2).await;

    let resp = send(
        &state,
        reg,
        discount(
            cart,
            ManualDiscountKind::PercentCart,
            1000,
            None,
            "damaged box",
        ),
    )
    .await;
    assert!(resp.success, "{:?}", resp.errors);
    let cart = resp.payload.expect("cart");
    assert_eq!(cart["subtotal_cents"], 2000);
    assert_eq!(cart["discount_cents"], 200, "10% of 2000");
    assert_eq!(cart["total_cents"], 1800);
    assert_eq!(cart["manual_discounts"][0]["reason"], "damaged box");
}

#[tokio::test]
async fn fixed_discounts_are_capped_at_what_they_discount() {
    let (state, item) = setup(10).await;
    let reg = Uuid::new_v4();
    let (cart, line) = cart_with(&state, reg, item, 1).await;

    let resp = send(
        &state,
        reg,
        discount(
            cart,
            ManualDiscountKind::FixedItem,
            5000,
            Some(line),
            "price match",
        ),
    )
    .await;
    assert!(resp.success, "{:?}", resp.errors);
    let cart = resp.payload.expect("cart");
    assert_eq!(
        cart["discount_cents"], 1000,
        "never more than the line is worth"
    );
    assert_eq!(cart["total_cents"], 0);
}

#[tokio::test]
async fn manual_discount_requires_a_reason_and_a_line_for_item_kinds() {
    let (state, item) = setup(10).await;
    let reg = Uuid::new_v4();
    let (cart, _) = cart_with(&state, reg, item, 1).await;

    let no_reason = send(
        &state,
        reg,
        discount(cart, ManualDiscountKind::FixedCart, 100, None, "  "),
    )
    .await;
    assert_eq!(code(&no_reason), "REASON_REQUIRED");

    let no_line = send(
        &state,
        reg,
        discount(cart, ManualDiscountKind::PercentItem, 100, None, "x"),
    )
    .await;
    assert_eq!(code(&no_line), "LINE_ID_REQUIRED");

    let bad_line = send(
        &state,
        reg,
        discount(
            cart,
            ManualDiscountKind::FixedItem,
            100,
            Some(Uuid::new_v4()),
            "x",
        ),
    )
    .await;
    assert_eq!(code(&bad_line), "LINE_NOT_FOUND");

    let zero = send(
        &state,
        reg,
        discount(cart, ManualDiscountKind::PercentCart, 0, None, "x"),
    )
    .await;
    assert_eq!(code(&zero), "ZERO_DISCOUNT");

    let missing_cart = send(
        &state,
        reg,
        discount(
            Uuid::new_v4(),
            ManualDiscountKind::FixedCart,
            100,
            None,
            "x",
        ),
    )
    .await;
    assert_eq!(code(&missing_cart), "CART_NOT_FOUND");
}

// ---------- list_parked_carts ----------

#[tokio::test]
async fn list_parked_carts_shows_the_store_or_one_register() {
    let (state, item) = setup(10).await;
    let front = Uuid::new_v4();
    let back = Uuid::new_v4();
    for register in [front, back] {
        let (cart, _) = cart_with(&state, register, item, 1).await;
        let parked = send(
            &state,
            register,
            PosCommand::ParkCart(ParkCartPayload {
                cart_id: cart,
                note: Some("customer went to ATM".into()),
            }),
        )
        .await;
        assert!(parked.success, "{:?}", parked.errors);
    }

    let all = send(
        &state,
        front,
        PosCommand::ListParkedCarts(ListParkedCartsPayload { register_id: None }),
    )
    .await;
    assert!(all.success, "{:?}", all.errors);
    assert_eq!(
        all.payload.expect("list").as_array().expect("array").len(),
        2
    );

    let only_back = send(
        &state,
        front,
        PosCommand::ListParkedCarts(ListParkedCartsPayload {
            register_id: Some(back),
        }),
    )
    .await;
    let only_back = only_back.payload.expect("list");
    let only_back = only_back.as_array().expect("array");
    assert_eq!(only_back.len(), 1);
    assert_eq!(only_back[0]["register_id"], back.to_string());
    assert_eq!(only_back[0]["total_cents"], 1000);
}

// ---------- transfer_stock / adjust_stock ----------

fn movement(item_id: Uuid, quantity_delta: i64, reason: &str) -> StockMovementPayload {
    StockMovementPayload {
        item_id,
        quantity_delta,
        reason: reason.into(),
        reference: Some("REF-1".into()),
    }
}

async fn available(state: &AppState, item: Uuid) -> i64 {
    get_inventory_state(&state.pool, STORE, item)
        .await
        .unwrap()
        .expect("tracked")
        .available_to_sell()
}

#[tokio::test]
async fn transfer_out_and_negative_adjustment_reduce_sellable_stock_and_are_queued_for_hq() {
    let (state, item) = setup(10).await;
    let reg = Uuid::new_v4();

    let transfer = send(
        &state,
        reg,
        PosCommand::TransferStock(movement(item, -3, "to store 12")),
    )
    .await;
    assert!(transfer.success, "{:?}", transfer.errors);
    let transfer = transfer.payload.expect("movement");
    assert_eq!(transfer["operation"], "transfer_stock");
    assert_eq!(transfer["reference"], "REF-1");
    assert_eq!(available(&state, item).await, 7);

    let adjust = send(
        &state,
        reg,
        PosCommand::AdjustStock(movement(item, -2, "shrinkage")),
    )
    .await;
    assert!(adjust.success, "{:?}", adjust.errors);
    assert_eq!(
        adjust.payload.expect("movement")["operation"],
        "adjust_stock"
    );
    assert_eq!(available(&state, item).await, 5);

    let queued: Vec<String> = sqlx::query_scalar("SELECT payload FROM outbox")
        .fetch_all(&state.pool)
        .await
        .unwrap();
    let operations: Vec<String> = queued
        .iter()
        .map(|p| serde_json::from_str::<Value>(p).unwrap())
        .filter(|p| p["event_type"] == "stock.movement")
        .map(|p| p["movement"]["operation"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(operations, vec!["transfer_stock", "adjust_stock"]);
}

#[tokio::test]
async fn a_stock_movement_needs_a_quantity_and_a_reason() {
    let (state, item) = setup(10).await;
    let reg = Uuid::new_v4();

    for command in [
        PosCommand::AdjustStock(movement(item, 0, "count")),
        PosCommand::TransferStock(movement(item, 4, "   ")),
    ] {
        let resp = send(&state, reg, command).await;
        assert_eq!(code(&resp), "INVALID_STOCK_MOVEMENT");
    }
    assert_eq!(
        available(&state, item).await,
        10,
        "rejected movements change nothing"
    );
}
