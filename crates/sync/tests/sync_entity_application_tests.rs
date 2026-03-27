//! Behavioral tests: sync entity application persists data to storage tables.
//!
//! These tests verify that after `run_sync_ndjson`, each synced entity's data
//! actually appears in the corresponding DB table — not just that checkpoints advance.

use apex_edge_contracts::{
    CatalogItem, Category, CouponDefinition, Customer, DocumentType, InventoryLevel, PriceBook,
    PriceBookEntry, PrintTemplateConfig, TaxRule,
};
use apex_edge_storage::{
    get_catalog_item, get_coupon_definition_by_code, get_print_template, insert_catalog_item,
    list_catalog_items, list_categories, list_price_book_entries, list_tax_rules, run_migrations,
    search_customers,
};
use apex_edge_sync::{run_sync_ndjson, SyncEntityConfig, SyncSourceConfig};
use axum::body::Body;
use axum::http::Response;
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::net::TcpListener;
use uuid::Uuid;

const STORE_ID: Uuid = Uuid::nil();

fn encode_payload(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

fn ndjson_body(payloads: &[Vec<u8>]) -> String {
    let mut lines = vec![format!("{{\"total\":{}}}", payloads.len())];
    for p in payloads {
        lines.push(format!("\"{}\"", encode_payload(p)));
    }
    lines.join("\n")
}

async fn start_entity_server(entity: &'static str, body: String) -> (u16, String) {
    let app = Router::new().route(
        &format!("/sync/ndjson/{entity}"),
        get(move || {
            let body = body.clone();
            async move {
                Response::builder()
                    .header("content-type", "application/x-ndjson")
                    .body(Body::from(body))
                    .unwrap()
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let base = format!("http://127.0.0.1:{port}");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;
    (port, base)
}

#[tokio::test]
async fn sync_catalog_items_are_applied_to_db() {
    let item = CatalogItem {
        id: Uuid::parse_str("a1a1a1a1-a1a1-a1a1-a1a1-a1a1a1a1a1a1").unwrap(),
        sku: "SYNC-CAT-001".into(),
        name: "Synced Catalog Product".into(),
        description: Some("A product from sync".into()),
        category_id: Uuid::nil(),
        tax_category_id: Uuid::nil(),
        modifiers: vec![],
        is_active: true,
        title: Some("Synced Catalog Product".into()),
        brand: Some("Demo Brand".into()),
        caption: Some("Demo Caption".into()),
        external_identifiers: Some(apex_edge_contracts::ExternalIdentifiers {
            sku: Some("SYNC-CAT-001".into()),
            gtin: Some("1234567890123".into()),
            upc: None,
            ean13: None,
            jan: None,
            isbn: None,
        }),
        images: Some(vec![apex_edge_contracts::ProductImage {
            url: "https://example.com/p1.jpg".into(),
            title: Some("Main".into()),
            identifier: None,
            is_main: Some(true),
            alt_text: None,
            dominant_color: None,
            width: Some(640),
            height: Some(640),
            aspect_ratio: Some(1.0),
            tags: None,
        }]),
        is_preorder: None,
        online_from: None,
        serialized_inventory: None,
        extended_attributes: None,
        variations: None,
        variation_attributes: None,
        version: 1,
    };
    let payload = serde_json::to_vec(&item).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("catalog", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "catalog".into(),
            path: "/sync/ndjson/catalog".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let (items, total) = list_catalog_items(&pool, STORE_ID, None, None, 10, 0)
        .await
        .expect("list_catalog_items");
    assert_eq!(total, 1, "one catalog item should be stored");
    assert_eq!(items[0].sku, "SYNC-CAT-001");
    assert_eq!(items[0].name, "Synced Catalog Product");
    let raw = items[0]
        .raw_product_json
        .as_deref()
        .expect("raw product payload should be stored");
    let raw_json: serde_json::Value = serde_json::from_str(raw).expect("valid json");
    assert_eq!(raw_json["brand"], "Demo Brand");
    assert_eq!(raw_json["title"], "Synced Catalog Product");
}

#[tokio::test]
async fn sync_catalog_replaces_stale_items_for_store() {
    let stale_id = Uuid::parse_str("aaaaaaaa-0000-0000-0000-000000000000").unwrap();
    let synced_id = Uuid::parse_str("aaaaaaaa-1111-1111-1111-111111111111").unwrap();
    let synced = CatalogItem {
        id: synced_id,
        sku: "SYNC-CAT-NEW".into(),
        name: "Synced New Product".into(),
        description: Some("fresh snapshot row".into()),
        category_id: Uuid::nil(),
        tax_category_id: Uuid::nil(),
        modifiers: vec![],
        is_active: true,
        title: Some("Synced New Product".into()),
        brand: None,
        caption: None,
        external_identifiers: None,
        images: None,
        is_preorder: None,
        online_from: None,
        serialized_inventory: None,
        extended_attributes: None,
        variations: None,
        variation_attributes: None,
        version: 1,
    };
    let body = ndjson_body(&[serde_json::to_vec(&synced).unwrap()]);
    let (_, base_url) = start_entity_server("catalog", body).await;

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    insert_catalog_item(
        &pool,
        stale_id,
        STORE_ID,
        "STALE-001",
        "Stale Product",
        Uuid::nil(),
        Uuid::nil(),
    )
    .await
    .expect("insert stale catalog row");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "catalog".into(),
            path: "/sync/ndjson/catalog".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let (items, total) = list_catalog_items(&pool, STORE_ID, None, None, 50, 0)
        .await
        .expect("list_catalog_items");
    assert_eq!(
        total, 1,
        "stale catalog rows should be replaced by sync snapshot"
    );
    assert_eq!(items[0].id, synced_id);
    assert_eq!(items[0].sku, "SYNC-CAT-NEW");
}

#[tokio::test]
async fn sync_categories_are_applied_to_db() {
    let cat = Category {
        id: Uuid::parse_str("b2b2b2b2-b2b2-b2b2-b2b2-b2b2b2b2b2b2").unwrap(),
        name: "Synced Category".into(),
        parent_id: None,
        sort_order: 1,
        version: 1,
    };
    let payload = serde_json::to_vec(&cat).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("categories", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "categories".into(),
            path: "/sync/ndjson/categories".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let cats = list_categories(&pool, STORE_ID)
        .await
        .expect("list_categories");
    assert_eq!(cats.len(), 1, "one category should be stored");
    assert_eq!(cats[0].name, "Synced Category");
}

#[tokio::test]
async fn sync_price_book_entries_are_applied_to_db() {
    let item_id = Uuid::parse_str("c3c3c3c3-c3c3-c3c3-c3c3-c3c3c3c3c3c3").unwrap();
    let book = PriceBook {
        id: Uuid::parse_str("d4d4d4d4-d4d4-d4d4-d4d4-d4d4d4d4d4d4").unwrap(),
        name: "Default".into(),
        effective_from: chrono::Utc::now(),
        effective_until: None,
        entries: vec![PriceBookEntry {
            item_id,
            modifier_option_id: None,
            price_cents: 999,
            currency: "USD".into(),
        }],
        version: 1,
    };
    let payload = serde_json::to_vec(&book).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("price_book", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "price_book".into(),
            path: "/sync/ndjson/price_book".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let entries = list_price_book_entries(&pool, STORE_ID)
        .await
        .expect("list_price_book_entries");
    assert_eq!(entries.len(), 1, "one price book entry should be stored");
    assert_eq!(entries[0].item_id, item_id);
    assert_eq!(entries[0].price_cents, 999);
    assert_eq!(entries[0].currency, "USD");
}

#[tokio::test]
async fn sync_tax_rules_are_applied_to_db() {
    let rule = TaxRule {
        id: Uuid::parse_str("e5e5e5e5-e5e5-e5e5-e5e5-e5e5e5e5e5e5").unwrap(),
        tax_category_id: Uuid::nil(),
        rate_bps: 850,
        name: "Synced Tax".into(),
        inclusive: false,
        version: 1,
    };
    let payload = serde_json::to_vec(&rule).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("tax_rules", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "tax_rules".into(),
            path: "/sync/ndjson/tax_rules".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let rules = list_tax_rules(&pool, STORE_ID)
        .await
        .expect("list_tax_rules");
    assert_eq!(rules.len(), 1, "one tax rule should be stored");
    assert_eq!(rules[0].rate_bps, 850);
    assert_eq!(rules[0].name, "Synced Tax");
}

#[tokio::test]
async fn sync_customers_are_applied_to_db() {
    let customer = Customer {
        id: Uuid::parse_str("f6f6f6f6-f6f6-f6f6-f6f6-f6f6f6f6f6f6").unwrap(),
        code: "SYNCCUST01".into(),
        name: "Synced Customer".into(),
        email: Some("synced@test.local".into()),
        version: 1,
    };
    let payload = serde_json::to_vec(&customer).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("customers", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "customers".into(),
            path: "/sync/ndjson/customers".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let results = search_customers(&pool, STORE_ID, "SYNCCUST01")
        .await
        .expect("search_customers");
    assert_eq!(results.len(), 1, "one customer should be stored");
    assert_eq!(results[0].code, "SYNCCUST01");
    assert_eq!(results[0].name, "Synced Customer");
    assert_eq!(results[0].email.as_deref(), Some("synced@test.local"));
}

#[tokio::test]
async fn sync_inventory_levels_are_applied_to_db() {
    let item_id = Uuid::parse_str("a1a1a1a1-a1a1-a1a1-a1a1-a1a1a1a1a1a1").unwrap();
    let level = InventoryLevel {
        item_id,
        available_qty: 42,
        is_available: true,
        image_urls: vec![
            "https://example.com/img1.jpg".into(),
            "https://example.com/img2.jpg".into(),
        ],
        version: 1,
    };
    let payload = serde_json::to_vec(&level).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("inventory", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    insert_catalog_item(
        &pool,
        item_id,
        STORE_ID,
        "INV-TEST-001",
        "Inventory Test Product",
        Uuid::nil(),
        Uuid::nil(),
    )
    .await
    .expect("insert catalog item");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "inventory".into(),
            path: "/sync/ndjson/inventory".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let item = get_catalog_item(&pool, STORE_ID, item_id)
        .await
        .expect("get catalog item")
        .expect("item exists");
    assert_eq!(
        item.available_qty,
        Some(42),
        "available_qty should be synced"
    );
    assert_eq!(
        item.is_available,
        Some(true),
        "is_available should be synced"
    );
    assert_eq!(item.image_urls.len(), 2, "image_urls should be synced");
    assert!(item
        .image_urls
        .contains(&"https://example.com/img1.jpg".to_string()));
}

#[tokio::test]
async fn sync_catalog_persists_is_active_flag() {
    let item_id = Uuid::parse_str("b2b2b2b2-b2b2-b2b2-b2b2-000000000001").unwrap();
    let item = CatalogItem {
        id: item_id,
        sku: "INACTIVE-001".into(),
        name: "Inactive Product".into(),
        description: None,
        category_id: Uuid::nil(),
        tax_category_id: Uuid::nil(),
        modifiers: vec![],
        is_active: false,
        title: Some("Inactive Product".into()),
        brand: None,
        caption: None,
        external_identifiers: None,
        images: None,
        is_preorder: None,
        online_from: None,
        serialized_inventory: None,
        extended_attributes: None,
        variations: None,
        variation_attributes: None,
        version: 1,
    };
    let payload = serde_json::to_vec(&item).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("catalog", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "catalog".into(),
            path: "/sync/ndjson/catalog".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let stored = get_catalog_item(&pool, STORE_ID, item_id)
        .await
        .expect("get catalog item")
        .expect("item exists");
    assert!(
        !stored.is_active,
        "is_active=false should be persisted from catalog sync"
    );
}

#[tokio::test]
async fn sync_print_templates_are_applied_to_db() {
    let template_id = Uuid::parse_str("a7a7a7a7-a7a7-a7a7-a7a7-a7a7a7a7a7a7").unwrap();
    let template = PrintTemplateConfig {
        id: template_id,
        document_type: DocumentType::CustomerReceipt,
        template_body:
            "<html><body>Sales Receipt {{order_id}} Total: {{total_cents}}</body></html>".into(),
        version: 1,
    };
    let payload = serde_json::to_vec(&template).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("print_templates", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "print_templates".into(),
            path: "/sync/ndjson/print_templates".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let row = get_print_template(&pool, STORE_ID, "customer_receipt")
        .await
        .expect("get_print_template")
        .expect("one template should be stored");
    assert_eq!(row.template_id, template_id);
    assert_eq!(row.document_type, "customer_receipt");
    assert!(row.template_body.contains("{{order_id}}"));
    assert_eq!(row.version, 1);
}

#[tokio::test]
async fn sync_gift_receipt_template_applied_to_db() {
    let template_id = Uuid::parse_str("b8b8b8b8-b8b8-b8b8-b8b8-b8b8b8b8b8b8").unwrap();
    let template = PrintTemplateConfig {
        id: template_id,
        document_type: DocumentType::GiftReceipt,
        template_body: "<html><body>Gift Receipt {{order_id}}</body></html>".into(),
        version: 1,
    };
    let payload = serde_json::to_vec(&template).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("print_templates", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "print_templates".into(),
            path: "/sync/ndjson/print_templates".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let row = get_print_template(&pool, STORE_ID, "gift_receipt")
        .await
        .expect("get_print_template")
        .expect("gift receipt template should be stored");
    assert_eq!(row.template_id, template_id);
    assert_eq!(row.document_type, "gift_receipt");
}

#[tokio::test]
async fn sync_coupons_are_applied_to_db() {
    let coupon = CouponDefinition {
        id: Uuid::parse_str("c9c9c9c9-c9c9-c9c9-c9c9-c9c9c9c9c9c9").unwrap(),
        code: "SYNC-CPN-01".into(),
        promo_id: Uuid::new_v4(),
        max_redemptions_total: Some(500),
        max_redemptions_per_customer: Some(1),
        valid_from: chrono::Utc::now() - chrono::Duration::minutes(5),
        valid_until: Some(chrono::Utc::now() + chrono::Duration::minutes(5)),
        version: 1,
    };
    let payload = serde_json::to_vec(&coupon).unwrap();
    let body = ndjson_body(&[payload]);

    let (_, base_url) = start_entity_server("coupons", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "coupons".into(),
            path: "/sync/ndjson/coupons".into(),
        }],
    };
    run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await
    .expect("run_sync_ndjson");

    let row = get_coupon_definition_by_code(&pool, STORE_ID, "sync-cpn-01")
        .await
        .expect("get coupon")
        .expect("coupon should exist");
    assert_eq!(row.code, "SYNC-CPN-01");
    assert_eq!(row.max_redemptions_total, Some(500));
    assert_eq!(row.max_redemptions_per_customer, Some(1));
}

#[tokio::test]
async fn sync_invalid_catalog_payload_returns_error() {
    // Serve malformed JSON (not a valid CatalogItem)
    let bad_payload = b"not valid json at all!".to_vec();
    let body = ndjson_body(&[bad_payload]);

    let (_, base_url) = start_entity_server("catalog", body).await;
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url,
        entities: vec![SyncEntityConfig {
            entity: "catalog".into(),
            path: "/sync/ndjson/catalog".into(),
        }],
    };
    let result = run_sync_ndjson(
        &reqwest::Client::new(),
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        STORE_ID,
    )
    .await;
    assert!(
        result.is_err(),
        "sync with invalid payload should return an error"
    );
}
