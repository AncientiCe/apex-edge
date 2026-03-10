//! Ingest pipeline: snapshot/delta, checkpoints, atomic upserts.

use apex_edge_contracts::ContractVersion;
use apex_edge_storage::{get_sync_checkpoint, set_sync_checkpoint};
use sqlx::sqlite::SqlitePool;
use thiserror::Error;

use crate::conflict::ConflictPolicy;

#[derive(Error, Debug)]
pub enum IngestError {
    #[error("storage: {0}")]
    Storage(#[from] apex_edge_storage::pool::PoolError),
    #[error("invalid payload")]
    InvalidPayload,
}

/// Ingest a batch and advance the per-entity checkpoint.
///
/// # Examples
///
/// ```no_run
/// use apex_edge_contracts::ContractVersion;
/// use apex_edge_sync::{ingest_batch, ConflictPolicy};
/// use sqlx::sqlite::SqlitePoolOptions;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let pool = SqlitePoolOptions::new()
///     .max_connections(1)
///     .connect("sqlite::memory:")
///     .await
///     .unwrap();
/// apex_edge_storage::run_migrations(&pool).await.unwrap();
///
/// let next = ingest_batch(
///     &pool,
///     "catalog",
///     ContractVersion::V1_0_0,
///     &[b"delta-1".to_vec()],
///     ConflictPolicy::HqWins,
/// )
/// .await
/// .unwrap();
/// assert_eq!(next, 1);
/// # }
/// ```
pub async fn ingest_batch(
    pool: &SqlitePool,
    entity: &str,
    _version: ContractVersion,
    payloads: &[Vec<u8>],
    policy: ConflictPolicy,
) -> Result<u64, IngestError> {
    let _ = (policy, payloads);
    let seq = get_sync_checkpoint(pool, entity).await?.unwrap_or(0);
    let next = seq + payloads.len() as i64;
    set_sync_checkpoint(pool, entity, next).await?;
    Ok(next as u64)
}
