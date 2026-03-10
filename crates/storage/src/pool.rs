//! DB pool (SQLite for edge, optional Postgres).

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PoolError {
    #[error("connection: {0}")]
    Connection(#[from] sqlx::Error),
}

pub async fn create_sqlite_pool(path: &str) -> Result<SqlitePool, PoolError> {
    let pool = SqlitePoolOptions::new()
        .acquire_timeout(Duration::from_secs(5))
        .connect(path)
        .await?;
    Ok(pool)
}
