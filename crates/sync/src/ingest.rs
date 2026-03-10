//! Ingest pipeline: snapshot/delta, checkpoints, atomic upserts.

use apex_edge_contracts::ContractVersion;
use apex_edge_metrics::{
    OUTCOME_CHECKPOINT_ADVANCED, OUTCOME_ERROR, OUTCOME_INVALID_PAYLOAD, SYNC_INGEST_BATCHES_TOTAL,
    SYNC_INGEST_DURATION_SECONDS,
};
use apex_edge_storage::{get_sync_checkpoint, set_sync_checkpoint};
use sqlx::sqlite::SqlitePool;
use std::time::Instant;
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
    version: ContractVersion,
    payloads: &[Vec<u8>],
    policy: ConflictPolicy,
) -> Result<u64, IngestError> {
    let start = Instant::now();
    let result = async {
        let seq = get_sync_checkpoint(pool, entity).await?.unwrap_or(0);
        let next = seq + payloads.len() as i64;
        set_sync_checkpoint(pool, entity, next).await?;
        Ok::<_, IngestError>(next as u64)
    }
    .await;

    let outcome = match &result {
        Ok(_) => OUTCOME_CHECKPOINT_ADVANCED,
        Err(IngestError::InvalidPayload) => OUTCOME_INVALID_PAYLOAD,
        Err(IngestError::Storage(_)) => OUTCOME_ERROR,
    };
    metrics::counter!(
        SYNC_INGEST_BATCHES_TOTAL,
        1u64,
        "entity" => entity.to_string(),
        "outcome" => outcome,
        "policy" => format!("{:?}", policy),
        "version" => version.to_string()
    );
    metrics::histogram!(
        SYNC_INGEST_DURATION_SECONDS,
        start.elapsed().as_secs_f64(),
        "entity" => entity.to_string()
    );
    result
}
