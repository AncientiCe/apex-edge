use apex_edge_contracts::ContractVersion;
use apex_edge_storage::{get_sync_checkpoint, run_migrations};
use apex_edge_sync::{ingest_batch, ConflictPolicy};
use sqlx::sqlite::SqlitePoolOptions;

#[tokio::test]
async fn ingest_batch_advances_checkpoints() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let next = ingest_batch(
        &pool,
        "catalog",
        ContractVersion::V1_0_0,
        &[b"a".to_vec(), b"b".to_vec()],
        ConflictPolicy::HqWins,
    )
    .await
    .expect("first ingest");
    assert_eq!(next, 2);

    let next2 = ingest_batch(
        &pool,
        "catalog",
        ContractVersion::V1_0_0,
        &[b"c".to_vec()],
        ConflictPolicy::HqWins,
    )
    .await
    .expect("second ingest");
    assert_eq!(next2, 3);
    assert_eq!(
        get_sync_checkpoint(&pool, "catalog")
            .await
            .expect("checkpoint"),
        Some(3)
    );
}
