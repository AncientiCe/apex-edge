use apex_edge_contracts::InventoryLevel;
use apex_edge_storage::*;
use chrono::Utc;
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

async fn test_pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    pool
}

#[tokio::test]
async fn storage_roundtrip_for_core_tables() {
    let pool = test_pool().await;

    let key = Uuid::new_v4();
    set_response(&pool, key, "ok")
        .await
        .expect("set idempotency");
    let got = get_response(&pool, key).await.expect("get idempotency");
    assert_eq!(got.as_deref(), Some("ok"));

    let outbox_id = Uuid::new_v4();
    insert_outbox(&pool, outbox_id, r#"{"order":"1"}"#)
        .await
        .expect("insert outbox");
    let pending = fetch_pending_outbox(&pool, 10).await.expect("fetch outbox");
    assert_eq!(pending.len(), 1);
    mark_delivered(&pool, outbox_id)
        .await
        .expect("mark delivered");
    assert!(fetch_pending_outbox(&pool, 10)
        .await
        .expect("fetch outbox after deliver")
        .is_empty());

    let order_id = Uuid::new_v4();
    let doc_id = Uuid::new_v4();
    enqueue_document(
        &pool,
        doc_id,
        "receipt",
        Some(order_id),
        None,
        Uuid::new_v4(),
        r#"{"k":"v"}"#,
    )
    .await
    .expect("enqueue doc");
    mark_generated(&pool, doc_id, "text/plain", "content")
        .await
        .expect("mark generated");
    let doc = get_document(&pool, doc_id)
        .await
        .expect("get doc")
        .expect("doc exists");
    assert_eq!(doc.status, "generated");
    assert_eq!(doc.content.as_deref(), Some("content"));
    assert_eq!(
        list_documents_for_order(&pool, order_id)
            .await
            .expect("list docs")
            .len(),
        1
    );

    set_sync_checkpoint(&pool, "catalog", 9)
        .await
        .expect("set checkpoint");
    assert_eq!(
        get_sync_checkpoint(&pool, "catalog")
            .await
            .expect("get checkpoint"),
        Some(9)
    );

    let cart_id = Uuid::new_v4();
    let store_id = Uuid::new_v4();
    let register_id = Uuid::new_v4();
    save_cart(
        &pool,
        cart_id,
        store_id,
        register_id,
        &apex_edge_contracts::CartStateKind::Itemized,
        &serde_json::json!({"line_count": 1}),
    )
    .await
    .expect("save cart");
    let loaded = load_cart(&pool, cart_id)
        .await
        .expect("load cart")
        .expect("cart row");
    assert_eq!(loaded.id, cart_id);
    assert_eq!(loaded.store_id, store_id);
}

#[tokio::test]
async fn seed_demo_data_populates_enough_catalog_and_customers() {
    let pool = test_pool().await;
    let store_id = Uuid::from_u128(1);
    let summary = seed_demo_data(&pool, store_id)
        .await
        .expect("seed demo data");
    assert!(summary.categories >= 6, "expected several categories");
    assert!(summary.products >= 120, "expected large catalog");
    assert!(summary.customers >= 80, "expected many customers");

    let product_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM catalog_items WHERE store_id = ?")
            .bind(store_id.to_string())
            .fetch_one(&pool)
            .await
            .expect("product count");
    assert!(product_count.0 >= 120);

    let customer_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM customers WHERE store_id = ?")
            .bind(store_id.to_string())
            .fetch_one(&pool)
            .await
            .expect("customer count");
    assert!(customer_count.0 >= 80);
}

// --- Inventory / availability ---

#[tokio::test]
async fn replace_inventory_levels_updates_availability_fields() {
    let pool = test_pool().await;
    let store_id = Uuid::from_u128(1);
    let item_id = Uuid::from_u128(0xAA01);

    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "INV-001",
        "Inventory Item",
        Uuid::nil(),
        Uuid::nil(),
    )
    .await
    .expect("insert catalog item");

    let levels = vec![InventoryLevel {
        item_id,
        available_qty: 15,
        is_available: true,
        image_urls: vec!["https://img.example.com/a.jpg".into()],
        version: 1,
    }];
    replace_inventory_levels(&pool, store_id, &levels)
        .await
        .expect("replace inventory levels");

    let row = get_catalog_item(&pool, store_id, item_id)
        .await
        .expect("get catalog item")
        .expect("item exists");
    assert_eq!(row.available_qty, Some(15));
    assert_eq!(row.is_available, Some(true));
    assert_eq!(row.image_urls, vec!["https://img.example.com/a.jpg"]);
}

#[tokio::test]
async fn replace_inventory_levels_clears_previous_data_for_store() {
    let pool = test_pool().await;
    let store_id = Uuid::from_u128(2);
    let item_id = Uuid::from_u128(0xAA02);

    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "INV-002",
        "Item 2",
        Uuid::nil(),
        Uuid::nil(),
    )
    .await
    .expect("insert catalog item");

    let first = vec![InventoryLevel {
        item_id,
        available_qty: 99,
        is_available: true,
        image_urls: vec![],
        version: 1,
    }];
    replace_inventory_levels(&pool, store_id, &first)
        .await
        .expect("first replace");

    let updated = vec![InventoryLevel {
        item_id,
        available_qty: 3,
        is_available: true,
        image_urls: vec!["https://new.img/x.jpg".into()],
        version: 2,
    }];
    replace_inventory_levels(&pool, store_id, &updated)
        .await
        .expect("second replace");

    let row = get_catalog_item(&pool, store_id, item_id)
        .await
        .expect("get catalog item")
        .expect("item");
    assert_eq!(row.available_qty, Some(3));
    assert_eq!(row.image_urls, vec!["https://new.img/x.jpg"]);
}

#[tokio::test]
async fn catalog_item_is_active_flag_is_persisted_on_replace() {
    let pool = test_pool().await;
    let store_id = Uuid::from_u128(3);
    let item_id = Uuid::from_u128(0xAA03);
    let item = apex_edge_contracts::CatalogItem {
        id: item_id,
        sku: "INACTIVE-TEST".into(),
        name: "Inactive Test Item".into(),
        description: None,
        category_id: Uuid::nil(),
        tax_category_id: Uuid::nil(),
        modifiers: vec![],
        is_active: false,
        version: 1,
    };
    replace_catalog_items(&pool, store_id, &[item])
        .await
        .expect("replace catalog items");

    let row = get_catalog_item(&pool, store_id, item_id)
        .await
        .expect("get catalog item")
        .expect("item exists");
    assert!(!row.is_active, "is_active=false should be stored");
}

// --- Latest-only sync status persistence ---

#[tokio::test]
async fn sync_status_upsert_and_read_latest_run() {
    let pool = test_pool().await;
    let started = Utc::now();

    upsert_latest_sync_run(&pool, "running", Some(started), None, None)
        .await
        .expect("upsert run");

    let run = get_latest_sync_run(&pool).await.expect("get latest run");
    let run = run.expect("one row");
    assert_eq!(run.state, "running");
    assert!(run.started_at.is_some());
    assert_eq!(run.finished_at, None);
    assert_eq!(run.last_error, None);

    upsert_latest_sync_run(&pool, "success", Some(started), Some(Utc::now()), None)
        .await
        .expect("upsert run success");
    let run2 = get_latest_sync_run(&pool)
        .await
        .expect("get latest run")
        .unwrap();
    assert_eq!(run2.state, "success");
    assert!(run2.finished_at.is_some());
}

// --- Print templates ---

#[tokio::test]
async fn print_template_upsert_and_get_by_store_and_document_type() {
    let pool = test_pool().await;
    let store_id = Uuid::from_u128(1);
    let template_id = Uuid::from_u128(0xBB01);

    upsert_print_template(
        &pool,
        store_id,
        "customer_receipt",
        template_id,
        "<html><body>Receipt {{order_id}}</body></html>",
        1,
    )
    .await
    .expect("upsert_print_template");

    let row = get_print_template(&pool, store_id, "customer_receipt")
        .await
        .expect("get_print_template")
        .expect("template exists");
    assert_eq!(row.template_id, template_id);
    assert_eq!(row.document_type, "customer_receipt");
    assert!(row.template_body.contains("{{order_id}}"));

    // Upsert again (replace); version and body can change.
    upsert_print_template(
        &pool,
        store_id,
        "customer_receipt",
        template_id,
        "<html><body>Receipt {{order_id}} v2</body></html>",
        2,
    )
    .await
    .expect("upsert again");

    let row2 = get_print_template(&pool, store_id, "customer_receipt")
        .await
        .expect("get")
        .expect("template exists");
    assert_eq!(row2.version, 2);
    assert!(row2.template_body.contains("v2"));

    // Different document_type is separate row.
    upsert_print_template(
        &pool,
        store_id,
        "gift_receipt",
        Uuid::from_u128(0xBB02),
        "<html>Gift</html>",
        1,
    )
    .await
    .expect("upsert gift_receipt");
    let gift = get_print_template(&pool, store_id, "gift_receipt")
        .await
        .expect("get")
        .expect("gift template exists");
    assert_eq!(gift.document_type, "gift_receipt");
}

#[tokio::test]
async fn sync_status_upsert_and_read_entity_statuses() {
    let pool = test_pool().await;
    let now = Utc::now();

    upsert_entity_sync_status(&pool, "catalog", 100, Some(500), Some(20.0), now, "syncing")
        .await
        .expect("upsert entity");
    upsert_entity_sync_status(&pool, "price_book", 50, Some(50), Some(100.0), now, "done")
        .await
        .expect("upsert entity 2");

    let entities = get_entity_sync_statuses(&pool)
        .await
        .expect("get entity statuses");
    assert_eq!(entities.len(), 2);
    let catalog = entities.iter().find(|e| e.entity == "catalog").unwrap();
    assert_eq!(catalog.current, 100);
    assert_eq!(catalog.total, Some(500));
    assert_eq!(catalog.percent, Some(20.0));
    assert_eq!(catalog.status, "syncing");
}

#[tokio::test]
async fn auth_storage_pairing_device_and_session_roundtrip() {
    let pool = test_pool().await;
    let store_id = Uuid::from_u128(0xABC1);
    let now = Utc::now();
    let expires = now + chrono::Duration::minutes(5);

    let pairing_id = create_device_pairing_code(&pool, store_id, "code-hash", "admin", expires, 3)
        .await
        .expect("create pairing code");
    let pairing = get_pairing_code_by_hash(&pool, "code-hash")
        .await
        .expect("fetch pairing")
        .expect("pairing exists");
    assert_eq!(pairing.id, pairing_id);
    assert_eq!(pairing.max_attempts, 3);

    increment_pairing_code_attempts(&pool, pairing_id)
        .await
        .expect("increment attempts");
    let pairing_after = get_pairing_code_by_hash(&pool, "code-hash")
        .await
        .expect("fetch pairing")
        .expect("pairing exists");
    assert_eq!(pairing_after.attempts, 1);

    let device_id = Uuid::new_v4();
    create_trusted_device(
        &pool,
        device_id,
        store_id,
        "iPad-1",
        Some("ios"),
        "device-secret-hash",
    )
    .await
    .expect("create trusted device");
    let device = get_trusted_device(&pool, device_id)
        .await
        .expect("get device")
        .expect("device exists");
    assert_eq!(device.device_name, "iPad-1");
    assert_eq!(device.secret_hash, "device-secret-hash");
    assert!(device.revoked_at.is_none());

    consume_pairing_code(&pool, pairing_id, device_id)
        .await
        .expect("consume pairing");
    let pairing_consumed = get_pairing_code_by_hash(&pool, "code-hash")
        .await
        .expect("fetch pairing")
        .expect("pairing exists");
    assert!(pairing_consumed.consumed_at.is_some());

    upsert_associate_identity(
        &pool,
        "associate-1",
        store_id,
        Some("Associate One"),
        Some("a1@example.com"),
        r#"{"role":"cashier"}"#,
    )
    .await
    .expect("upsert associate identity");

    let session_id = Uuid::new_v4();
    create_auth_session(
        &pool,
        session_id,
        "associate-1",
        store_id,
        device_id,
        now + chrono::Duration::minutes(5),
        now + chrono::Duration::hours(8),
    )
    .await
    .expect("create session");
    let sess = get_auth_session(&pool, session_id)
        .await
        .expect("get session")
        .expect("session exists");
    assert_eq!(sess.associate_id, "associate-1");
    assert_eq!(sess.device_id, device_id);
    assert!(sess.revoked_at.is_none());

    revoke_auth_session(&pool, session_id)
        .await
        .expect("revoke session");
    let sess_after = get_auth_session(&pool, session_id)
        .await
        .expect("get session")
        .expect("session exists");
    assert!(sess_after.revoked_at.is_some());
}
