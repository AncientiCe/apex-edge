use apex_edge_api::{
    get_document, handle_pos_command, health, list_order_documents, ready, AppState,
};
use apex_edge_contracts::{ContractVersion, CreateCartPayload, PosCommand, PosRequestEnvelope};
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
