use apex_edge_api::{handle_pos_command, AppState};
use apex_edge_contracts::{ContractVersion, CreateCartPayload, PosCommand, PosRequestEnvelope};
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use axum::{extract::State, Json};
use uuid::Uuid;

#[tokio::test]
async fn identical_concurrent_commands_execute_once() {
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let state = AppState::new(pool.clone(), Uuid::nil());
    let request = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: Uuid::nil(),
        register_id: Uuid::new_v4(),
        payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    };
    let (a, b) = tokio::join!(
        handle_pos_command(State(state.clone()), None, Json(request.clone())),
        handle_pos_command(State(state), None, Json(request))
    );
    assert!(a.0.success && b.0.success);
    assert_eq!(
        a.0.payload, b.0.payload,
        "same request must replay the same cart"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM carts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
