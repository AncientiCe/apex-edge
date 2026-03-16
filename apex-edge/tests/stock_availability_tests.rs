//! Integration tests for stock/availability enforcement and product availability in the catalog API.

use apex_edge::build_router;
use apex_edge_contracts::{
    AddLineItemPayload, ContractVersion, CreateCartPayload, InventoryLevel, PosCommand,
    PosRequestEnvelope,
};
use apex_edge_storage::{
    insert_catalog_item, insert_price_book_entry, insert_tax_rule, replace_catalog_items,
    replace_inventory_levels, run_migrations,
};
use axum::http::StatusCode;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::net::TcpListener;
use uuid::Uuid;

const STORE_ID: Uuid = Uuid::nil();
const REGISTER_ID: Uuid = Uuid::nil();

async fn start_app() -> (u16, sqlx::SqlitePool) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let app = build_router(
        pool.clone(),
        STORE_ID,
        None,
        vec![],
        apex_edge_api::AuthSettings::default(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = reqwest::Client::new();
    for _ in 0..30 {
        if client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .map(|r| r.status() == StatusCode::OK)
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    (port, pool)
}

async fn pos_command(port: u16, cmd: PosCommand) -> serde_json::Value {
    let client = reqwest::Client::new();
    let envelope = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: cmd,
    };
    client
        .post(format!("http://127.0.0.1:{port}/pos/command"))
        .json(&envelope)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json")
}

async fn create_cart(port: u16) -> String {
    let res = pos_command(
        port,
        PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    )
    .await;
    res["payload"]["cart_id"]
        .as_str()
        .expect("cart_id")
        .to_string()
}

// ---- helpers to seed data ----

fn item_id() -> Uuid {
    Uuid::from_u128(0xFFFF_0001)
}

async fn seed_catalog_item(pool: &sqlx::SqlitePool) {
    let id = item_id();
    let tax_id = Uuid::from_u128(0x1111);
    insert_tax_rule(pool, Uuid::new_v4(), STORE_ID, tax_id, 0, "No tax", false)
        .await
        .expect("insert tax rule");
    insert_catalog_item(
        pool,
        id,
        STORE_ID,
        "TEST-001",
        "Test Item",
        Uuid::nil(),
        tax_id,
    )
    .await
    .expect("insert catalog item");
    insert_price_book_entry(pool, STORE_ID, id, None, 500, "USD")
        .await
        .expect("insert price book entry");
}

// ---- stock enforcement tests ----

#[tokio::test]
async fn add_to_cart_succeeds_when_inventory_not_tracked() {
    // Items with no inventory level set (available_qty = NULL) are allowed — untracked stock.
    let (port, pool) = start_app().await;
    seed_catalog_item(&pool).await;

    let cart_id = create_cart(port).await;
    let res = pos_command(
        port,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id: Uuid::parse_str(&cart_id).unwrap(),
            item_id: item_id(),
            modifier_option_ids: vec![],
            quantity: 2,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;

    assert_eq!(
        res["success"].as_bool(),
        Some(true),
        "should succeed when inventory not tracked: {:?}",
        res
    );
}

#[tokio::test]
async fn add_to_cart_blocked_when_item_is_inactive() {
    let (port, pool) = start_app().await;
    let id = item_id();
    let tax_id = Uuid::from_u128(0x2222);
    insert_tax_rule(&pool, Uuid::new_v4(), STORE_ID, tax_id, 0, "No tax", false)
        .await
        .expect("insert tax rule");

    let inactive_item = apex_edge_contracts::CatalogItem {
        id,
        sku: "INACTIVE-001".into(),
        name: "Inactive Item".into(),
        description: None,
        category_id: Uuid::nil(),
        tax_category_id: tax_id,
        modifiers: vec![],
        is_active: false,
        version: 1,
    };
    replace_catalog_items(&pool, STORE_ID, &[inactive_item])
        .await
        .expect("replace catalog items");
    insert_price_book_entry(&pool, STORE_ID, id, None, 500, "USD")
        .await
        .expect("insert price book entry");

    let cart_id = create_cart(port).await;
    let res = pos_command(
        port,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id: Uuid::parse_str(&cart_id).unwrap(),
            item_id: id,
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;

    assert_eq!(
        res["success"].as_bool(),
        Some(false),
        "should be blocked when is_active=false: {:?}",
        res
    );
    let error_code = res["errors"][0]["code"].as_str().unwrap_or("");
    assert_eq!(
        error_code, "OUT_OF_STOCK",
        "expected OUT_OF_STOCK error: {:?}",
        res
    );
}

#[tokio::test]
async fn add_to_cart_blocked_when_out_of_stock() {
    let (port, pool) = start_app().await;
    seed_catalog_item(&pool).await;

    let levels = vec![InventoryLevel {
        item_id: item_id(),
        available_qty: 0,
        is_available: false,
        image_urls: vec![],
        version: 1,
    }];
    replace_inventory_levels(&pool, STORE_ID, &levels)
        .await
        .expect("replace inventory levels");

    let cart_id = create_cart(port).await;
    let res = pos_command(
        port,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id: Uuid::parse_str(&cart_id).unwrap(),
            item_id: item_id(),
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;

    assert_eq!(
        res["success"].as_bool(),
        Some(false),
        "should be blocked when available_qty=0: {:?}",
        res
    );
    let error_code = res["errors"][0]["code"].as_str().unwrap_or("");
    assert_eq!(
        error_code, "OUT_OF_STOCK",
        "expected OUT_OF_STOCK: {:?}",
        res
    );
}

#[tokio::test]
async fn add_to_cart_blocked_when_quantity_exceeds_available_stock() {
    let (port, pool) = start_app().await;
    seed_catalog_item(&pool).await;

    let levels = vec![InventoryLevel {
        item_id: item_id(),
        available_qty: 3,
        is_available: true,
        image_urls: vec![],
        version: 1,
    }];
    replace_inventory_levels(&pool, STORE_ID, &levels)
        .await
        .expect("replace inventory levels");

    let cart_id = create_cart(port).await;
    let res = pos_command(
        port,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id: Uuid::parse_str(&cart_id).unwrap(),
            item_id: item_id(),
            modifier_option_ids: vec![],
            quantity: 5,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;

    assert_eq!(
        res["success"].as_bool(),
        Some(false),
        "should be blocked when qty > available: {:?}",
        res
    );
    let error_code = res["errors"][0]["code"].as_str().unwrap_or("");
    assert_eq!(
        error_code, "INSUFFICIENT_STOCK",
        "expected INSUFFICIENT_STOCK: {:?}",
        res
    );
}

#[tokio::test]
async fn add_to_cart_succeeds_when_sufficient_stock() {
    let (port, pool) = start_app().await;
    seed_catalog_item(&pool).await;

    let levels = vec![InventoryLevel {
        item_id: item_id(),
        available_qty: 10,
        is_available: true,
        image_urls: vec![],
        version: 1,
    }];
    replace_inventory_levels(&pool, STORE_ID, &levels)
        .await
        .expect("replace inventory levels");

    let cart_id = create_cart(port).await;
    let res = pos_command(
        port,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id: Uuid::parse_str(&cart_id).unwrap(),
            item_id: item_id(),
            modifier_option_ids: vec![],
            quantity: 3,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;

    assert_eq!(
        res["success"].as_bool(),
        Some(true),
        "should succeed when qty <= available: {:?}",
        res
    );
}

// ---- product availability in API ----

#[tokio::test]
async fn product_search_includes_availability_fields() {
    let (port, pool) = start_app().await;
    seed_catalog_item(&pool).await;

    let levels = vec![InventoryLevel {
        item_id: item_id(),
        available_qty: 7,
        is_available: true,
        image_urls: vec!["https://img.example.com/prod.jpg".into()],
        version: 1,
    }];
    replace_inventory_levels(&pool, STORE_ID, &levels)
        .await
        .expect("replace inventory levels");

    let client = reqwest::Client::new();
    let res: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/catalog/products"))
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");

    let items = res["items"].as_array().expect("items array");
    assert!(!items.is_empty(), "should return items");
    let item = &items[0];
    assert!(
        item.get("is_active").is_some(),
        "product should have is_active field: {:?}",
        item
    );
    assert!(
        item.get("available_qty").is_some(),
        "product should have available_qty field: {:?}",
        item
    );
    assert!(
        item.get("image_urls").is_some(),
        "product should have image_urls field: {:?}",
        item
    );
    assert_eq!(item["available_qty"].as_i64(), Some(7));
    let images = item["image_urls"].as_array().expect("image_urls array");
    assert_eq!(images.len(), 1);
}

#[tokio::test]
async fn product_by_id_returns_availability() {
    let (port, pool) = start_app().await;
    seed_catalog_item(&pool).await;

    let levels = vec![InventoryLevel {
        item_id: item_id(),
        available_qty: 5,
        is_available: true,
        image_urls: vec![
            "https://img.example.com/a.jpg".into(),
            "https://img.example.com/b.jpg".into(),
        ],
        version: 1,
    }];
    replace_inventory_levels(&pool, STORE_ID, &levels)
        .await
        .expect("replace inventory levels");

    let client = reqwest::Client::new();
    let id = item_id();
    let url = format!("http://127.0.0.1:{port}/catalog/products/{id}");
    let res = client.get(&url).send().await.expect("request");
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["id"].as_str(), Some(id.to_string().as_str()));
    assert_eq!(body["available_qty"].as_i64(), Some(5));
    let images = body["image_urls"].as_array().expect("image_urls");
    assert_eq!(images.len(), 2);
}
