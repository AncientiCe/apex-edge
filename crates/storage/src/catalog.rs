//! Catalog and reference data (ingested by sync). Stub for schema.

use crate::pool::PoolError;
use sqlx::SqlitePool;

pub async fn upsert_catalog_item(_pool: &SqlitePool, _data: &[u8]) -> Result<(), PoolError> {
    Ok(())
}
