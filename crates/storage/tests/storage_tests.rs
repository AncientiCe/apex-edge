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
