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
    // SQLx expects a SQLite URL (e.g. `sqlite:apex_edge.db`, `sqlite::memory:`).
    // For ergonomics and OS-agnostic config, accept plain file paths and prefix them.
    let url = if path.starts_with("sqlite:") {
        path.to_string()
    } else {
        format!("sqlite:{path}")
    };
    let pool = SqlitePoolOptions::new()
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await?;
    Ok(pool)
}
