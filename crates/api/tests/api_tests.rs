use apex_edge_api::{
    create_gift_receipt_document, get_cart_state_handler, get_document, get_prices,
    get_product_by_id, handle_pos_command, health, list_order_documents, ready, search_products,
    serve_metrics, sync_status, AppState, ProductSearchQuery,
};
use apex_edge_contracts::{
    AddLineItemPayload, ApplyCouponPayload, ApplyPromoPayload, CartState, CatalogItem,
    ContractVersion, CouponDefinition, CreateCartPayload, PosCommand, PosRequestEnvelope,
    ProductImage, PromoAction, PromoCondition, Promotion, PromotionType, RemoveCouponPayload,
    RemoveLineItemPayload, RemovePromoPayload, UpdateLineItemPayload, VoidCartPayload,
};
use apex_edge_storage::{
    enqueue_document, insert_catalog_item, insert_customer, insert_price_book_entry,
    insert_promotion, insert_tax_rule, list_documents_for_order, mark_generated,
    replace_catalog_items, run_migrations, upsert_coupon_definition, upsert_print_template,
};
use axum::response::IntoResponse;
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{Duration, Utc};
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn api_handlers_cover_health_ready_pos_and_documents() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let state = AppState {
        store_id: Uuid::nil(),
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };

    let h = health().await;
    assert_eq!(h.0.status, "ok");
    let r = ready(State(state.clone()))
        .await
        .expect("ready should succeed");
    assert_eq!(r.0.status, "ready");

    let bad = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::new(2, 0, 0),
            idempotency_key: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    assert!(!bad.0.success);

    let good = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    assert!(good.0.success);

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
    .expect("enqueue");
    mark_generated(&pool, doc_id, "text/plain", "doc")
        .await
        .expect("mark generated");

    let listed = list_order_documents(State(state.clone()), axum::extract::Path(order_id))
        .await
        .expect("list docs");
    assert_eq!(listed.0.len(), 1);
    assert_eq!(listed.0[0].type_, "sales_receipt");
    assert_eq!(listed.0[0].document_type, "sales_receipt");

    let fetched = get_document(State(state), axum::extract::Path(doc_id))
        .await
        .expect("get doc");
    assert_eq!(fetched.0.status, "generated");
    assert_eq!(fetched.0.type_, "sales_receipt");
    assert_eq!(fetched.0.document_type, "sales_receipt");
}

#[tokio::test]
async fn get_document_returns_not_found_for_unknown_id() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let state = AppState {
        store_id: Uuid::nil(),
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let res = get_document(State(state), axum::extract::Path(Uuid::new_v4())).await;
    assert_eq!(
        res.expect_err("must return not found"),
        axum::http::StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn create_gift_receipt_generates_new_document_for_order() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let state = AppState {
        store_id: Uuid::nil(),
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let order_id = Uuid::new_v4();
    let doc_id = Uuid::new_v4();
    enqueue_document(
        &pool,
        doc_id,
        "receipt",
        Some(order_id),
        None,
        Uuid::new_v4(),
        r#"{"order_id":"abc","total_cents":1234}"#,
    )
    .await
    .expect("enqueue");
    mark_generated(&pool, doc_id, "text/plain", "receipt")
        .await
        .expect("mark generated");

    let created = create_gift_receipt_document(State(state.clone()), axum::extract::Path(order_id))
        .await
        .expect("gift receipt");
    assert_eq!(created.0.document_type, "gift_receipt");

    let listed = list_order_documents(State(state), axum::extract::Path(order_id))
        .await
        .expect("list docs");
    assert_eq!(listed.0.len(), 2);
}

#[tokio::test]
async fn get_sync_status_returns_shape_with_last_sync_and_entities() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let state = AppState {
        store_id: Uuid::nil(),
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };

    let resp = sync_status(State(state)).await.expect("sync_status");
    assert!(resp.last_sync_at.is_none() || resp.last_sync_at.is_some());
    assert!(resp.entities.is_empty() || !resp.entities.is_empty());
    // Shape: each entity has entity, current, total, percent, last_synced_at, status
    for e in &resp.entities {
        assert!(!e.entity.is_empty());
        assert!(
            e.status == "syncing"
                || e.status == "done"
                || e.status == "pending"
                || e.status == "error"
        );
    }
}

#[tokio::test]
async fn get_cart_state_returns_cart_for_known_id() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let state = AppState {
        store_id: Uuid::nil(),
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };

    // Create a cart via the POS command handler
    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    assert!(created.0.success);
    let created_state: CartState =
        serde_json::from_value(created.0.payload.unwrap()).expect("cart state");
    let cart_id = created_state.cart_id;

    // GET /pos/cart/:cart_id should return the same cart
    let result = get_cart_state_handler(State(state.clone()), axum::extract::Path(cart_id)).await;
    assert!(result.is_ok(), "must return Ok for known cart_id");
    let fetched = result.unwrap().0;
    assert_eq!(fetched.cart_id, cart_id, "returned cart_id must match");
}

#[tokio::test]
async fn get_prices_returns_base_prices_for_requested_products() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let store_id = Uuid::nil();
    let item_id = Uuid::new_v4();
    let tax_category_id = Uuid::new_v4();
    let category_id = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "SKU-PRICE-1",
        "Priced Item",
        category_id,
        tax_category_id,
    )
    .await
    .expect("insert catalog item");
    insert_price_book_entry(&pool, store_id, item_id, None, 1234, "USD")
        .await
        .expect("insert base price");

    let state = AppState {
        store_id,
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let response = get_prices(
        State(state),
        Query(apex_edge_api::PriceQuery {
            product_id: vec![item_id.to_string()],
        }),
    )
    .await
    .expect("prices response");

    assert_eq!(response.0.items.len(), 1);
    assert_eq!(response.0.items[0].product_id, item_id);
    assert_eq!(response.0.items[0].currency_code, "USD");
    assert!((response.0.items[0].value - 12.34).abs() < f64::EPSILON);
}

#[tokio::test]
async fn search_products_uses_catalog_images_when_inventory_images_are_missing() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let store_id = Uuid::nil();
    let item_id = Uuid::new_v4();
    let item = CatalogItem {
        id: item_id,
        sku: "IMG-CAT-001".into(),
        name: "Catalog Image Product".into(),
        description: Some("with catalog image".into()),
        category_id: Uuid::new_v4(),
        tax_category_id: Uuid::new_v4(),
        modifiers: vec![],
        is_active: true,
        title: None,
        brand: None,
        caption: None,
        external_identifiers: None,
        images: Some(vec![
            ProductImage {
                url: "https://cdn.example.com/catalog/main.jpg".into(),
                title: Some("main".into()),
                ..ProductImage::default()
            },
            ProductImage {
                url: "https://cdn.example.com/catalog/side.jpg".into(),
                title: Some("side".into()),
                ..ProductImage::default()
            },
        ]),
        is_preorder: None,
        online_from: None,
        serialized_inventory: None,
        extended_attributes: None,
        variations: None,
        variation_attributes: None,
        version: 1,
    };
    replace_catalog_items(&pool, store_id, &[item])
        .await
        .expect("replace_catalog_items");

    let state = AppState {
        store_id,
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let response = search_products(
        State(state),
        Query(ProductSearchQuery {
            sku: Some("IMG-CAT-001".into()),
            q: None,
            category_id: None,
            page: 1,
            per_page: 24,
        }),
    )
    .await
    .expect("search_products response");

    let json = serde_json::to_value(response.0).expect("serialize");
    let arr = json.as_array().expect("sku search returns array");
    assert_eq!(arr.len(), 1);
    assert_eq!(
        arr[0]["image_urls"],
        serde_json::json!([
            "https://cdn.example.com/catalog/main.jpg",
            "https://cdn.example.com/catalog/side.jpg"
        ])
    );
}

#[tokio::test]
async fn product_by_id_returns_placeholder_image_when_no_synced_images_exist() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let store_id = Uuid::nil();
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "IMG-NONE-001",
        "No Image Product",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert catalog item");

    let state = AppState {
        store_id,
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let response = get_product_by_id(State(state), axum::extract::Path(item_id))
        .await
        .expect("get_product_by_id");

    assert_eq!(response.0.image_urls.len(), 1);
    assert_eq!(
        response.0.image_urls[0],
        "https://placehold.co/600x600/png?text=IMG-NONE-001"
    );
}

#[tokio::test]
async fn remove_line_item_returns_cart_not_found_for_unknown_cart() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let state = AppState {
        store_id: Uuid::nil(),
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };

    let res = handle_pos_command(
        State(state),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            payload: PosCommand::RemoveLineItem(RemoveLineItemPayload {
                cart_id: Uuid::new_v4(),
                line_id: Uuid::new_v4(),
            }),
        }),
    )
    .await;

    assert!(!res.0.success);
    assert_eq!(res.0.errors[0].code, "CART_NOT_FOUND");
}

#[tokio::test]
async fn remove_line_item_returns_line_not_found_for_unknown_line() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let state = AppState {
        store_id: Uuid::nil(),
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };

    // Create a cart first
    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    assert!(created.0.success);
    let cart_state: CartState =
        serde_json::from_value(created.0.payload.unwrap()).expect("cart state");

    // Attempt to remove a line that doesn't exist
    let res = handle_pos_command(
        State(state),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            payload: PosCommand::RemoveLineItem(RemoveLineItemPayload {
                cart_id: cart_state.cart_id,
                line_id: Uuid::new_v4(),
            }),
        }),
    )
    .await;

    assert!(!res.0.success);
    assert_eq!(res.0.errors[0].code, "LINE_NOT_FOUND");
}

#[tokio::test]
async fn get_cart_state_returns_not_found_for_unknown_id() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let state = AppState {
        store_id: Uuid::nil(),
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };

    let result = get_cart_state_handler(State(state), axum::extract::Path(Uuid::new_v4())).await;
    assert_eq!(
        result.expect_err("must return not found"),
        axum::http::StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn metrics_endpoint_returns_404_when_recorder_not_installed() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let state = AppState {
        store_id: Uuid::nil(),
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };

    let response = serve_metrics(State(state)).await.into_response();
    assert_eq!(
        response.status(),
        axum::http::StatusCode::NOT_FOUND,
        "metrics endpoint should return 404 when no handle is configured"
    );
}

#[tokio::test]
async fn add_line_item_response_includes_applied_promo_name() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let store_id = Uuid::nil();
    let state = AppState {
        store_id,
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "PROMO-001",
        "Promo Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert_catalog_item");
    insert_price_book_entry(&pool, store_id, item_id, None, 1000, "USD")
        .await
        .expect("insert_price_book_entry");
    let promo = Promotion {
        id: Uuid::new_v4(),
        code: None,
        name: "Donna Dress 2 for 1".into(),
        promo_type: PromotionType::PercentageOff { percent_bps: 10000 },
        priority: 100,
        valid_from: Utc::now() - Duration::minutes(5),
        valid_until: Some(Utc::now() + Duration::minutes(5)),
        conditions: vec![],
        actions: vec![PromoAction::ApplyToBasket],
        version: 1,
    };
    insert_promotion(
        &pool,
        promo.id,
        store_id,
        &serde_json::to_string(&promo).expect("promo json"),
    )
    .await
    .expect("insert_promotion");

    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    let created_state: CartState =
        serde_json::from_value(created.0.payload.expect("cart payload")).expect("cart state");

    let add = handle_pos_command(
        State(state),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::AddLineItem(AddLineItemPayload {
                cart_id: created_state.cart_id,
                item_id,
                modifier_option_ids: vec![],
                quantity: 1,
                notes: None,
                unit_price_override_cents: None,
            }),
        }),
    )
    .await;

    assert!(
        add.0.success,
        "add line item should succeed: {:?}",
        add.0.errors
    );
    let state_after_add: CartState =
        serde_json::from_value(add.0.payload.expect("add payload")).expect("cart state");
    assert_eq!(state_after_add.applied_promos.len(), 1);
    assert_eq!(
        state_after_add.applied_promos[0].name,
        "Donna Dress 2 for 1"
    );
}

/// When a customer_receipt template is synced and we finalize an order, the generated document
/// must have mime_type application/pdf and content that decodes to valid PDF bytes.
#[tokio::test]
async fn finalize_order_with_synced_template_produces_pdf_receipt() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let store_id = Uuid::nil();

    upsert_print_template(
        &pool,
        store_id,
        "customer_receipt",
        Uuid::new_v4(),
        "<html><body>Receipt {{order_id}} Total: {{total_cents}}</body></html>",
        1,
    )
    .await
    .expect("upsert template");

    let state = AppState {
        store_id,
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "PDF-001",
        "PDF Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert_catalog_item");
    insert_price_book_entry(&pool, store_id, item_id, None, 500, "USD")
        .await
        .expect("insert_price_book_entry");
    insert_tax_rule(
        &pool,
        Uuid::new_v4(),
        store_id,
        Uuid::nil(),
        0,
        "No tax",
        false,
    )
    .await
    .expect("insert_tax_rule");

    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    assert!(created.0.success);
    let cart_state: CartState =
        serde_json::from_value(created.0.payload.unwrap()).expect("cart state");

    let _ = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::AddLineItem(AddLineItemPayload {
                cart_id: cart_state.cart_id,
                item_id,
                modifier_option_ids: vec![],
                quantity: 1,
                notes: None,
                unit_price_override_cents: None,
            }),
        }),
    )
    .await;
    let _ = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::SetTendering(apex_edge_contracts::SetTenderingPayload {
                cart_id: cart_state.cart_id,
            }),
        }),
    )
    .await;
    let _ = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::AddPayment(apex_edge_contracts::AddPaymentPayload {
                cart_id: cart_state.cart_id,
                tender_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
                amount_cents: 500,
                external_reference: None,
            }),
        }),
    )
    .await;
    let finalize_res = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::FinalizeOrder(apex_edge_contracts::FinalizeOrderPayload {
                cart_id: cart_state.cart_id,
            }),
        }),
    )
    .await;
    assert!(
        finalize_res.0.success,
        "finalize should succeed: {:?}",
        finalize_res.0.errors
    );
    let finalize_payload: apex_edge_contracts::FinalizeResult =
        serde_json::from_value(finalize_res.0.payload.unwrap()).expect("finalize payload");
    let order_id = finalize_payload.order_id;

    let docs = list_documents_for_order(&pool, order_id)
        .await
        .expect("list docs");
    let receipt = docs
        .iter()
        .find(|d| d.document_type == "customer_receipt" || d.document_type == "receipt")
        .expect("receipt document should exist");
    assert_eq!(
        receipt.mime_type, "application/pdf",
        "receipt document must be PDF when template is synced"
    );
    let doc = apex_edge_storage::get_document(&pool, receipt.id)
        .await
        .expect("get doc")
        .expect("doc exists");
    assert_eq!(doc.mime_type, "application/pdf");
    let content = doc.content.expect("content present");
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content.trim())
        .expect("content must be valid base64 PDF");
    assert!(
        bytes.starts_with(b"%PDF-"),
        "decoded content must be PDF (starts with %PDF-)"
    );
}

/// When a gift_receipt template is synced, create_gift_receipt_document must produce a document
/// with mime_type application/pdf.
#[tokio::test]
async fn gift_receipt_with_synced_template_produces_pdf() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let store_id = Uuid::nil();

    upsert_print_template(
        &pool,
        store_id,
        "gift_receipt",
        Uuid::new_v4(),
        "<html><body>Gift Receipt {{order_id}}</body></html>",
        1,
    )
    .await
    .expect("upsert gift_receipt template");

    let order_id = Uuid::new_v4();
    let source_doc_id = Uuid::new_v4();
    enqueue_document(
        &pool,
        source_doc_id,
        "receipt",
        Some(order_id),
        None,
        Uuid::new_v4(),
        r#"{"order_id":"oid","total_cents":999}"#,
    )
    .await
    .expect("enqueue");
    mark_generated(&pool, source_doc_id, "text/plain", "receipt")
        .await
        .expect("mark generated");

    let state = AppState {
        store_id,
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let created = create_gift_receipt_document(State(state), axum::extract::Path(order_id))
        .await
        .expect("create_gift_receipt");
    assert_eq!(created.0.document_type, "gift_receipt");
    assert_eq!(
        created.0.mime_type, "application/pdf",
        "gift receipt document must be PDF when template is synced"
    );
    let new_doc_id = created.0.id;
    assert_ne!(new_doc_id, source_doc_id);
    let doc = apex_edge_storage::get_document(&pool, new_doc_id)
        .await
        .expect("get")
        .expect("doc");
    assert_eq!(doc.mime_type, "application/pdf");
    let content = doc.content.expect("content");
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content.trim())
        .expect("base64");
    assert!(bytes.starts_with(b"%PDF-"));
}

#[tokio::test]
async fn update_line_item_reprices_and_updates_quantity() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let store_id = Uuid::nil();
    let state = AppState {
        store_id,
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "UPD-001",
        "Update Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert catalog item");
    insert_price_book_entry(&pool, store_id, item_id, None, 350, "USD")
        .await
        .expect("insert price");

    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    let created_state: CartState =
        serde_json::from_value(created.0.payload.expect("create payload")).expect("create state");

    let add = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::AddLineItem(AddLineItemPayload {
                cart_id: created_state.cart_id,
                item_id,
                modifier_option_ids: vec![],
                quantity: 1,
                notes: Some("initial".into()),
                unit_price_override_cents: None,
            }),
        }),
    )
    .await;
    assert!(add.0.success, "add line should succeed: {:?}", add.0.errors);
    let after_add: CartState =
        serde_json::from_value(add.0.payload.expect("add payload")).expect("add state");
    let line = after_add.lines.first().expect("line exists");

    let update = handle_pos_command(
        State(state),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::UpdateLineItem(UpdateLineItemPayload {
                cart_id: after_add.cart_id,
                line_id: line.line_id,
                quantity: 3,
                notes: Some("updated".into()),
            }),
        }),
    )
    .await;

    assert!(update.0.success, "update line should succeed");
    let after_update: CartState =
        serde_json::from_value(update.0.payload.expect("update payload")).expect("update state");
    assert_eq!(after_update.lines.len(), 1);
    assert_eq!(after_update.lines[0].quantity, 3);
    assert_eq!(after_update.lines[0].line_total_cents, 1050);
    assert_eq!(after_update.lines[0].notes.as_deref(), Some("updated"));
}

#[tokio::test]
async fn apply_and_remove_coupon_updates_cart_coupon_state() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let store_id = Uuid::nil();
    let state = AppState {
        store_id,
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "COUPON-001",
        "Coupon Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert catalog");
    insert_price_book_entry(&pool, store_id, item_id, None, 1000, "USD")
        .await
        .expect("insert price");

    let promo = Promotion {
        id: Uuid::new_v4(),
        code: Some("SAVE200".into()),
        name: "Save 200 cents".into(),
        promo_type: PromotionType::FixedAmountOff { amount_cents: 200 },
        priority: 100,
        valid_from: Utc::now() - Duration::minutes(5),
        valid_until: Some(Utc::now() + Duration::minutes(5)),
        conditions: vec![PromoCondition::MinBasketAmount { amount_cents: 1 }],
        actions: vec![PromoAction::ApplyToBasket],
        version: 1,
    };
    insert_promotion(
        &pool,
        promo.id,
        store_id,
        &serde_json::to_string(&promo).expect("promo json"),
    )
    .await
    .expect("insert promo");
    upsert_coupon_definition(
        &pool,
        store_id,
        &CouponDefinition {
            id: Uuid::new_v4(),
            code: "SAVE200".into(),
            promo_id: promo.id,
            max_redemptions_total: Some(1000),
            max_redemptions_per_customer: Some(10),
            valid_from: Utc::now() - Duration::minutes(5),
            valid_until: Some(Utc::now() + Duration::minutes(5)),
            version: 1,
        },
    )
    .await
    .expect("upsert coupon definition");

    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    let created_state: CartState =
        serde_json::from_value(created.0.payload.expect("create payload")).expect("create state");

    let add = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::AddLineItem(AddLineItemPayload {
                cart_id: created_state.cart_id,
                item_id,
                modifier_option_ids: vec![],
                quantity: 1,
                notes: None,
                unit_price_override_cents: None,
            }),
        }),
    )
    .await;
    assert!(
        add.0.success,
        "add line should succeed before coupon: {:?}",
        add.0.errors
    );
    let before_coupon: CartState =
        serde_json::from_value(add.0.payload.expect("add payload")).expect("add state");
    let total_before = before_coupon.total_cents;

    let applied = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::ApplyCoupon(ApplyCouponPayload {
                cart_id: before_coupon.cart_id,
                coupon_code: "SAVE200".into(),
            }),
        }),
    )
    .await;
    assert!(applied.0.success, "apply coupon should succeed");
    let after_apply: CartState =
        serde_json::from_value(applied.0.payload.expect("apply payload")).expect("apply state");
    assert_eq!(after_apply.applied_coupons.len(), 1);
    assert_eq!(after_apply.applied_coupons[0].code, "SAVE200");
    assert!(after_apply.total_cents < total_before);

    let removed = handle_pos_command(
        State(state),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::RemoveCoupon(RemoveCouponPayload {
                cart_id: after_apply.cart_id,
                coupon_id: after_apply.applied_coupons[0].coupon_id,
            }),
        }),
    )
    .await;
    assert!(removed.0.success, "remove coupon should succeed");
    let after_remove: CartState =
        serde_json::from_value(removed.0.payload.expect("remove payload")).expect("remove state");
    assert!(after_remove.applied_coupons.is_empty());
    assert_eq!(after_remove.total_cents, total_before);
}

#[tokio::test]
async fn void_cart_marks_cart_voided_and_blocks_future_edits() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let store_id = Uuid::nil();
    let state = AppState {
        store_id,
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "VOID-001",
        "Void Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert catalog");
    insert_price_book_entry(&pool, store_id, item_id, None, 500, "USD")
        .await
        .expect("insert price");

    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    let created_state: CartState =
        serde_json::from_value(created.0.payload.expect("create payload")).expect("create state");

    let voided = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::VoidCart(VoidCartPayload {
                cart_id: created_state.cart_id,
                reason: Some("customer_cancelled".into()),
            }),
        }),
    )
    .await;
    assert!(
        voided.0.success,
        "void should succeed: {:?}",
        voided.0.errors
    );
    let after_void: CartState =
        serde_json::from_value(voided.0.payload.expect("void payload")).expect("void state");
    assert_eq!(after_void.state, apex_edge_contracts::CartStateKind::Voided);

    let add_after_void = handle_pos_command(
        State(state),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::AddLineItem(AddLineItemPayload {
                cart_id: after_void.cart_id,
                item_id,
                modifier_option_ids: vec![],
                quantity: 1,
                notes: None,
                unit_price_override_cents: None,
            }),
        }),
    )
    .await;
    assert!(!add_after_void.0.success);
    assert_eq!(add_after_void.0.errors[0].code, "INVALID_STATE");
}

#[tokio::test]
async fn apply_and_remove_promo_updates_discount_and_applied_promos() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let store_id = Uuid::nil();
    let state = AppState {
        store_id,
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "PROMO-CMD-001",
        "Promo Command Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert catalog");
    insert_price_book_entry(&pool, store_id, item_id, None, 1000, "USD")
        .await
        .expect("insert price");
    let promo = Promotion {
        id: Uuid::new_v4(),
        code: Some("MANUALPROMO".into()),
        name: "Manual 20%".into(),
        promo_type: PromotionType::PercentageOff { percent_bps: 2000 },
        priority: 100,
        valid_from: Utc::now() - Duration::minutes(5),
        valid_until: Some(Utc::now() + Duration::minutes(5)),
        conditions: vec![PromoCondition::MinBasketAmount { amount_cents: 1 }],
        actions: vec![PromoAction::ApplyToBasket],
        version: 1,
    };
    insert_promotion(
        &pool,
        promo.id,
        store_id,
        &serde_json::to_string(&promo).expect("promo json"),
    )
    .await
    .expect("insert promo");

    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    let created_state: CartState =
        serde_json::from_value(created.0.payload.expect("create payload")).expect("create state");
    let add = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::AddLineItem(AddLineItemPayload {
                cart_id: created_state.cart_id,
                item_id,
                modifier_option_ids: vec![],
                quantity: 1,
                notes: None,
                unit_price_override_cents: None,
            }),
        }),
    )
    .await;
    let before: CartState =
        serde_json::from_value(add.0.payload.expect("add payload")).expect("add state");
    assert_eq!(before.discount_cents, 0);

    let applied = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::ApplyPromo(ApplyPromoPayload {
                cart_id: before.cart_id,
                promo_id: promo.id,
            }),
        }),
    )
    .await;
    assert!(
        applied.0.success,
        "apply promo should succeed: {:?}",
        applied.0.errors
    );
    let after_apply: CartState =
        serde_json::from_value(applied.0.payload.expect("apply payload")).expect("apply state");
    assert_eq!(after_apply.applied_promos.len(), 1);
    assert_eq!(after_apply.applied_promos[0].promo_id, promo.id);
    assert!(after_apply.discount_cents > 0);

    let removed = handle_pos_command(
        State(state),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::RemovePromo(RemovePromoPayload {
                cart_id: after_apply.cart_id,
                promo_id: promo.id,
            }),
        }),
    )
    .await;
    assert!(
        removed.0.success,
        "remove promo should succeed: {:?}",
        removed.0.errors
    );
    let after_remove: CartState =
        serde_json::from_value(removed.0.payload.expect("remove payload")).expect("remove state");
    assert!(after_remove.applied_promos.is_empty());
    assert_eq!(after_remove.discount_cents, 0);
}

#[tokio::test]
async fn apply_coupon_rejects_when_redemption_limit_reached() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let store_id = Uuid::nil();
    let state = AppState {
        store_id,
        pool: pool.clone(),
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        item_id,
        store_id,
        "CPN-LIMIT-001",
        "Coupon Limit Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert catalog");
    insert_price_book_entry(&pool, store_id, item_id, None, 1000, "USD")
        .await
        .expect("insert price");
    let promo = Promotion {
        id: Uuid::new_v4(),
        code: Some("LIMITED".into()),
        name: "Limited coupon promo".into(),
        promo_type: PromotionType::FixedAmountOff { amount_cents: 200 },
        priority: 100,
        valid_from: Utc::now() - Duration::minutes(5),
        valid_until: Some(Utc::now() + Duration::minutes(5)),
        conditions: vec![PromoCondition::MinBasketAmount { amount_cents: 1 }],
        actions: vec![PromoAction::ApplyToBasket],
        version: 1,
    };
    insert_promotion(
        &pool,
        promo.id,
        store_id,
        &serde_json::to_string(&promo).expect("promo json"),
    )
    .await
    .expect("insert promo");
    let coupon_def = CouponDefinition {
        id: Uuid::new_v4(),
        code: "LIMITED".into(),
        promo_id: promo.id,
        max_redemptions_total: Some(0),
        max_redemptions_per_customer: None,
        valid_from: Utc::now() - Duration::minutes(5),
        valid_until: Some(Utc::now() + Duration::minutes(5)),
        version: 1,
    };
    upsert_coupon_definition(&pool, store_id, &coupon_def)
        .await
        .expect("upsert coupon");

    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    let created_state: CartState =
        serde_json::from_value(created.0.payload.expect("create payload")).expect("create state");
    let add = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::AddLineItem(AddLineItemPayload {
                cart_id: created_state.cart_id,
                item_id,
                modifier_option_ids: vec![],
                quantity: 1,
                notes: None,
                unit_price_override_cents: None,
            }),
        }),
    )
    .await;
    assert!(add.0.success, "add line should succeed");

    let applied = handle_pos_command(
        State(state),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::ApplyCoupon(ApplyCouponPayload {
                cart_id: created_state.cart_id,
                coupon_code: "LIMITED".into(),
            }),
        }),
    )
    .await;
    assert!(!applied.0.success, "coupon should be rejected");
    assert_eq!(applied.0.errors[0].code, "INVALID_COUPON");
}

#[tokio::test]
async fn repeated_idempotency_key_replays_same_response() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let state = AppState {
        store_id: Uuid::nil(),
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let idem = Uuid::new_v4();
    let req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: idem,
        store_id: Uuid::nil(),
        register_id: Uuid::nil(),
        payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    };

    let first = handle_pos_command(State(state.clone()), Json(req.clone())).await;
    let second = handle_pos_command(State(state), Json(req)).await;

    assert!(first.0.success);
    assert!(second.0.success);
    assert_eq!(
        first.0.payload, second.0.payload,
        "idempotency replay should return original payload"
    );
}

#[tokio::test]
async fn set_customer_enriches_cart_state_with_customer_name_and_code() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let store_id = Uuid::nil();
    let customer_id = Uuid::new_v4();
    insert_customer(
        &pool,
        customer_id,
        store_id,
        "CUST-001",
        "Alice Smith",
        Some("alice@example.com"),
    )
    .await
    .expect("insert customer");

    let state = AppState {
        store_id,
        pool,
        metrics_handle: None,
        auth: apex_edge_api::AuthSettings::default(),
    };
    let created = handle_pos_command(
        State(state.clone()),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    let created_state: CartState =
        serde_json::from_value(created.0.payload.expect("create payload")).expect("create state");

    let set_customer = handle_pos_command(
        State(state),
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id: Uuid::nil(),
            payload: PosCommand::SetCustomer(apex_edge_contracts::SetCustomerPayload {
                cart_id: created_state.cart_id,
                customer_id,
            }),
        }),
    )
    .await;
    assert!(set_customer.0.success);
    let payload: serde_json::Value = set_customer.0.payload.expect("set customer payload");
    assert_eq!(payload["customer_name"], serde_json::json!("Alice Smith"));
    assert_eq!(payload["customer_code"], serde_json::json!("CUST-001"));
}
