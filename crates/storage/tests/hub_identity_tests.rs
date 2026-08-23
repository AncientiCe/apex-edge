//! Hub identity: store_id and register_id must survive a restart.

use apex_edge_storage::{resolve_hub_identity, run_migrations};
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

async fn pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    pool
}

#[tokio::test]
async fn first_boot_persists_a_stable_identity() {
    let pool = pool().await;
    let first = resolve_hub_identity(&pool, None, None)
        .await
        .expect("first resolve");
    let second = resolve_hub_identity(&pool, None, None)
        .await
        .expect("second resolve");
    assert_eq!(first, second);
    assert_ne!(first.store_id, Uuid::nil());
    assert_ne!(first.register_id, Uuid::nil());
}

#[tokio::test]
async fn env_store_id_overrides_and_is_persisted() {
    let pool = pool().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();
    let written = resolve_hub_identity(&pool, Some(store), Some(register))
        .await
        .expect("write");
    assert_eq!(written.store_id, store);
    assert_eq!(written.register_id, register);
    let reread = resolve_hub_identity(&pool, None, None)
        .await
        .expect("reread");
    assert_eq!(reread.store_id, store);
    assert_eq!(reread.register_id, register);
}
