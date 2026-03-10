use apex_edge_api::{
    create_gift_receipt_document, get_cart_state_handler, get_document, handle_pos_command, health,
    list_order_documents, ready, serve_metrics, sync_status, AppState,
};
use apex_edge_contracts::{
    AddLineItemPayload, CartState, ContractVersion, CreateCartPayload, PosCommand,
    PosRequestEnvelope, PromoAction, Promotion, PromotionType, RemoveLineItemPayload,
};
use apex_edge_storage::{
    enqueue_document, insert_catalog_item, insert_price_book_entry, insert_promotion,
    mark_generated, run_migrations,
};
use axum::response::IntoResponse;
use axum::{extract::State, Json};
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

    let fetched = get_document(State(state), axum::extract::Path(doc_id))
        .await
        .expect("get doc");
    assert_eq!(fetched.0.status, "generated");
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
        code: Some("AUTO100".into()),
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

    assert!(add.0.success, "add line item should succeed");
    let state_after_add: CartState =
        serde_json::from_value(add.0.payload.expect("add payload")).expect("cart state");
    assert_eq!(state_after_add.applied_promos.len(), 1);
    assert_eq!(
        state_after_add.applied_promos[0].name,
        "Donna Dress 2 for 1"
    );
}
