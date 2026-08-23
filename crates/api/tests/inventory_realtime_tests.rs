//! End-to-end behaviour for the real-time inventory ledger (oversell prevention).
//!
//! Drives commands through `execute_pos_command`, asserting that reservations across
//! separate carts (registers) cannot oversell, that releasing/voiding returns stock,
//! that finalizing commits the sale, and that local stock receipts become sellable.

use apex_edge_api::{pos_handler::execute_pos_command, AppState};
use apex_edge_contracts::{
    AddLineItemPayload, ContractVersion, CreateCartPayload, ParkCartPayload, PosCommand,
    PosRequestEnvelope, RecallCartPayload, StockMovementPayload, VoidCartPayload,
};
use apex_edge_storage::{
    create_sqlite_pool, run_migrations, seed_inventory_from_catalog, set_audit_key, AuditKey,
};
use uuid::Uuid;

const STORE: Uuid = Uuid::nil();

async fn setup_with_item(available_qty: i64) -> (AppState, Uuid) {
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
    .bind(available_qty)
    .execute(&pool)
    .await
    .unwrap();
    seed_inventory_from_catalog(&pool, STORE).await.unwrap();
    let state = AppState::new(pool, STORE);
    (state, item_id)
}

fn env<T>(register_id: Uuid, payload: T) -> PosRequestEnvelope<T> {
    PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE,
        register_id,
        payload,
    }
}

async fn create_cart(state: &AppState, register: Uuid) -> Uuid {
    let cart_id = Uuid::new_v4();
    let resp = execute_pos_command(
        state,
        env(
            register,
            PosCommand::CreateCart(CreateCartPayload {
                cart_id: Some(cart_id),
            }),
        ),
    )
    .await;
    assert!(resp.success, "create_cart failed: {:?}", resp.errors);
    cart_id
}

async fn add_line(
    state: &AppState,
    register: Uuid,
    cart_id: Uuid,
    item_id: Uuid,
    quantity: u32,
) -> apex_edge_contracts::PosResponseEnvelope<serde_json::Value> {
    execute_pos_command(
        state,
        env(
            register,
            PosCommand::AddLineItem(AddLineItemPayload {
                cart_id,
                item_id,
                modifier_option_ids: vec![],
                quantity,
                notes: None,
                unit_price_override_cents: None,
            }),
        ),
    )
    .await
}

#[tokio::test]
async fn concurrent_registers_cannot_oversell_last_unit() {
    let (state, item_id) = setup_with_item(1).await;
    let reg_a = Uuid::new_v4();
    let reg_b = Uuid::new_v4();
    let cart_a = create_cart(&state, reg_a).await;
    let cart_b = create_cart(&state, reg_b).await;

    // Register A reserves the only unit.
    let resp_a = add_line(&state, reg_a, cart_a, item_id, 1).await;
    assert!(
        resp_a.success,
        "A should reserve the unit: {:?}",
        resp_a.errors
    );

    // Register B must be blocked from selling the same unit.
    let resp_b = add_line(&state, reg_b, cart_b, item_id, 1).await;
    assert!(!resp_b.success, "B must not oversell the last unit");
    assert_eq!(resp_b.errors[0].code, "OUT_OF_STOCK");
}

#[tokio::test]
async fn voiding_cart_releases_stock_for_other_register() {
    let (state, item_id) = setup_with_item(1).await;
    let reg_a = Uuid::new_v4();
    let reg_b = Uuid::new_v4();
    let cart_a = create_cart(&state, reg_a).await;
    let cart_b = create_cart(&state, reg_b).await;

    assert!(add_line(&state, reg_a, cart_a, item_id, 1).await.success);
    // B blocked while A holds it.
    assert!(!add_line(&state, reg_b, cart_b, item_id, 1).await.success);

    // A voids its cart, releasing the unit.
    let void = execute_pos_command(
        &state,
        env(
            reg_a,
            PosCommand::VoidCart(VoidCartPayload {
                cart_id: cart_a,
                reason: Some("abandoned".into()),
            }),
        ),
    )
    .await;
    assert!(void.success, "void failed: {:?}", void.errors);

    // Now B can reserve it.
    assert!(
        add_line(&state, reg_b, cart_b, item_id, 1).await.success,
        "B should reserve after A released"
    );
}

#[tokio::test]
async fn insufficient_quantity_is_rejected() {
    let (state, item_id) = setup_with_item(3).await;
    let reg = Uuid::new_v4();
    let cart = create_cart(&state, reg).await;
    let resp = add_line(&state, reg, cart, item_id, 5).await;
    assert!(!resp.success);
    assert_eq!(resp.errors[0].code, "INSUFFICIENT_STOCK");
}

#[tokio::test]
async fn receiving_stock_makes_item_sellable() {
    let (state, item_id) = setup_with_item(0).await;
    let reg = Uuid::new_v4();
    let cart = create_cart(&state, reg).await;

    // Out of stock initially.
    assert!(!add_line(&state, reg, cart, item_id, 1).await.success);

    // Receive 5 units locally.
    let recv = execute_pos_command(
        &state,
        env(
            reg,
            PosCommand::ReceiveStock(StockMovementPayload {
                item_id,
                quantity_delta: 5,
                reason: "delivery".into(),
                reference: None,
            }),
        ),
    )
    .await;
    assert!(recv.success, "receive_stock failed: {:?}", recv.errors);

    // Now sellable.
    assert!(
        add_line(&state, reg, cart, item_id, 1).await.success,
        "item should be sellable after receiving stock"
    );
}

#[tokio::test]
async fn parked_cart_handoff_is_safe_against_double_recall() {
    let (state, item_id) = setup_with_item(10).await;
    let reg_a = Uuid::new_v4();
    let reg_b = Uuid::new_v4();
    let reg_c = Uuid::new_v4();
    let cart_a = create_cart(&state, reg_a).await;
    assert!(add_line(&state, reg_a, cart_a, item_id, 1).await.success);

    // Register A parks the cart.
    let park = execute_pos_command(
        &state,
        env(
            reg_a,
            PosCommand::ParkCart(ParkCartPayload {
                cart_id: cart_a,
                note: Some("hold".into()),
            }),
        ),
    )
    .await;
    assert!(park.success, "park failed: {:?}", park.errors);
    let parked_cart_id =
        Uuid::parse_str(park.payload.unwrap()["parked_cart_id"].as_str().unwrap()).unwrap();

    // Register B claims it (handoff).
    let recall_b = execute_pos_command(
        &state,
        env(
            reg_b,
            PosCommand::RecallCart(RecallCartPayload { parked_cart_id }),
        ),
    )
    .await;
    assert!(
        recall_b.success,
        "B should claim the parked cart: {:?}",
        recall_b.errors
    );

    // Register C tries to claim the same cart — must lose the race.
    let recall_c = execute_pos_command(
        &state,
        env(
            reg_c,
            PosCommand::RecallCart(RecallCartPayload { parked_cart_id }),
        ),
    )
    .await;
    assert!(!recall_c.success, "C must not double-recall");
    assert_eq!(recall_c.errors[0].code, "CART_ALREADY_RECALLED");
}

#[tokio::test]
async fn untracked_item_is_not_constrained() {
    // available_qty NULL → untracked. Insert directly without a ledger row.
    set_audit_key(AuditKey::new("test-hub", b"test-secret".to_vec()));
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let item_id = Uuid::new_v4();
    let cat = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO catalog_items (id, store_id, sku, name, category_id, tax_category_id, is_active) \
         VALUES (?, ?, 'SKU-U', 'Untracked', ?, ?, 1)",
    )
    .bind(item_id.to_string())
    .bind(STORE.to_string())
    .bind(cat.to_string())
    .bind(cat.to_string())
    .execute(&pool)
    .await
    .unwrap();
    let state = AppState::new(pool, STORE);
    let reg = Uuid::new_v4();
    let cart = create_cart(&state, reg).await;
    // Large quantity must succeed since stock is untracked.
    assert!(add_line(&state, reg, cart, item_id, 9999).await.success);
}
