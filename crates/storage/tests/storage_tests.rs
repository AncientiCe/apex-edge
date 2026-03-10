use apex_edge_storage::*;
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
