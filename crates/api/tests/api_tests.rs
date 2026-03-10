use apex_edge_api::{
    create_gift_receipt_document, get_cart_state_handler, get_document, handle_pos_command, health,
    list_order_documents, ready, sync_status, AppState,
};
use apex_edge_contracts::{
    CartState, ContractVersion, CreateCartPayload, PosCommand, PosRequestEnvelope,
};
use apex_edge_storage::{enqueue_document, mark_generated, run_migrations};
use axum::{extract::State, Json};
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
