//! Chaos / business-continuity integration tests.
//!
//! These exercise edge behaviour that must hold when HQ/the WAN is unavailable or when the
//! hub crashes mid-cart:
//!   - HQ down: the store keeps selling and `/sync/status` reports degraded mode.
//!   - Oversell race: two registers racing for the last unit — only one wins.
//!   - Snapshot: `/pos/snapshot` returns full live state for resnapshot recovery.
//!   - Restart mid-cart: reservations persist across a process restart (crash recovery).

use apex_edge::{build_router, HubConfig};
use apex_edge_contracts::{
    AddLineItemPayload, ContractVersion, CreateCartPayload, InventoryLevel, PosCommand,
    PosRequestEnvelope,
};
use apex_edge_storage::{
    create_sqlite_pool, insert_catalog_item, insert_price_book_entry, insert_tax_rule,
    replace_inventory_levels, run_migrations,
};
use axum::http::StatusCode;
use sqlx::sqlite::SqlitePoolOptions;
use tempfile::TempDir;
use tokio::net::TcpListener;
use uuid::Uuid;

const STORE_ID: Uuid = Uuid::nil();
const REGISTER_ID: Uuid = Uuid::nil();

fn item_id() -> Uuid {
    Uuid::from_u128(0xFFFF_0042)
}

async fn serve(pool: sqlx::SqlitePool) -> u16 {
    let app = build_router(
        pool,
        HubConfig {
            store_id: STORE_ID,
            ..HubConfig::default()
        },
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = reqwest::Client::new();
    for _ in 0..40 {
        if client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .map(|r| r.status() == StatusCode::OK)
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
    }
    port
}

async fn mem_pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    pool
}

async fn seed_item(pool: &sqlx::SqlitePool, available_qty: i64) {
    let id = item_id();
    let tax_id = Uuid::from_u128(0x9001);
    insert_tax_rule(pool, Uuid::new_v4(), STORE_ID, tax_id, 0, "No tax", false)
        .await
        .expect("tax rule");
    insert_catalog_item(
        pool,
        id,
        STORE_ID,
        "CHAOS-001",
        "Chaos Item",
        Uuid::nil(),
        tax_id,
    )
    .await
    .expect("catalog item");
    insert_price_book_entry(pool, STORE_ID, id, None, 500, "USD")
        .await
        .expect("price");
    replace_inventory_levels(
        pool,
        STORE_ID,
        &[InventoryLevel {
            item_id: id,
            available_qty,
            is_available: available_qty > 0,
            image_urls: vec![],
            version: 1,
        }],
    )
    .await
    .expect("inventory level");
}

async fn pos_command(port: u16, register_id: Uuid, cmd: PosCommand) -> serde_json::Value {
    let envelope = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE_ID,
        register_id,
        payload: cmd,
    };
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/pos/command"))
        .json(&envelope)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json")
}

async fn create_cart(port: u16) -> Uuid {
    let res = pos_command(
        port,
        REGISTER_ID,
        PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    )
    .await;
    Uuid::parse_str(res["payload"]["cart_id"].as_str().expect("cart_id")).expect("uuid")
}

async fn add_line(port: u16, cart_id: Uuid, qty: u32) -> serde_json::Value {
    pos_command(
        port,
        REGISTER_ID,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id,
            item_id: item_id(),
            modifier_option_ids: vec![],
            quantity: qty,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await
}

#[tokio::test]
async fn hq_down_store_keeps_selling_and_status_is_degraded() {
    // No sync source is configured (HQ unreachable) and no successful sync recorded.
    let pool = mem_pool().await;
    seed_item(&pool, 5).await;
    let port = serve(pool).await;

    // Selling still works on the locally-seeded baseline.
    let cart = create_cart(port).await;
    let res = add_line(port, cart, 2).await;
    assert_eq!(
        res["success"].as_bool(),
        Some(true),
        "sale blocked: {res:?}"
    );

    // Status reports degraded mode (no trustworthy baseline yet).
    let status: serde_json::Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/sync/status"))
        .send()
        .await
        .expect("status req")
        .json()
        .await
        .expect("json");
    assert_eq!(status["degraded"].as_bool(), Some(true), "{status:?}");
    assert!(status["sync_staleness_seconds"].is_null());
}

#[tokio::test]
async fn two_registers_racing_for_last_unit_only_one_wins() {
    let pool = mem_pool().await;
    seed_item(&pool, 1).await;
    let port = serve(pool).await;

    let cart_a = create_cart(port).await;
    let cart_b = create_cart(port).await;

    // Fire both adds concurrently for the single available unit.
    let (ra, rb) = tokio::join!(add_line(port, cart_a, 1), add_line(port, cart_b, 1));
    let a_ok = ra["success"].as_bool() == Some(true);
    let b_ok = rb["success"].as_bool() == Some(true);
    assert!(
        a_ok ^ b_ok,
        "exactly one register must win the last unit: a={ra:?} b={rb:?}"
    );
}

#[tokio::test]
async fn snapshot_returns_live_stock_state() {
    let pool = mem_pool().await;
    seed_item(&pool, 4).await;
    let port = serve(pool).await;

    // Reserve 1 so available_to_sell becomes 3 (also lazily seeds the ledger).
    let cart = create_cart(port).await;
    assert_eq!(
        add_line(port, cart, 1).await["success"].as_bool(),
        Some(true)
    );

    let snap: serde_json::Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/pos/snapshot"))
        .send()
        .await
        .expect("snapshot req")
        .json()
        .await
        .expect("json");

    assert!(snap["seq"].as_u64().is_some());
    assert!(snap["registers"].is_array());
    assert!(snap["parked_carts"].is_array());
    let stock = snap["stock"].as_array().expect("stock array");
    let entry = stock
        .iter()
        .find(|e| e["item_id"].as_str() == Some(item_id().to_string().as_str()))
        .expect("seeded item present in snapshot");
    assert_eq!(entry["available_to_sell"].as_i64(), Some(3));
}

#[tokio::test]
async fn reservations_survive_a_process_restart_mid_cart() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("chaos.db").to_string_lossy().to_string();

    // First "process": seed, reserve 1 of 2, then drop the app/pool (simulated crash).
    {
        let pool = create_sqlite_pool(&db_path).await.expect("open");
        run_migrations(&pool).await.expect("migrate");
        seed_item(&pool, 2).await;
        let port = serve(pool.clone()).await;
        let cart = create_cart(port).await;
        assert_eq!(
            add_line(port, cart, 1).await["success"].as_bool(),
            Some(true)
        );
        // available_to_sell is now 1; reservation lives in SQLite.
        pool.close().await;
    }

    // Second "process": reopen the same on-disk DB and confirm the reservation persisted.
    {
        let pool = create_sqlite_pool(&db_path).await.expect("reopen");
        let port = serve(pool).await;
        let snap: serde_json::Value = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/pos/snapshot"))
            .send()
            .await
            .expect("snapshot req")
            .json()
            .await
            .expect("json");
        let stock = snap["stock"].as_array().expect("stock array");
        let entry = stock
            .iter()
            .find(|e| e["item_id"].as_str() == Some(item_id().to_string().as_str()))
            .expect("item present after restart");
        assert_eq!(
            entry["available_to_sell"].as_i64(),
            Some(1),
            "reservation must survive restart: {snap:?}"
        );
    }
}
