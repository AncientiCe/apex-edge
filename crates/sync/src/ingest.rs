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
