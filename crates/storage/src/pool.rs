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
    } else if path.starts_with('/') {
        // Absolute POSIX path
        format!("sqlite://{path}")
    } else if path.len() >= 3
        && path.as_bytes()[1] == b':'
        && (path.as_bytes()[2] == b'\\' || path.as_bytes()[2] == b'/')
    {
        // Absolute Windows path like C:\dir\file.db or C:/dir/file.db
        format!("sqlite:{path}")
    } else {
        // Relative path
        format!("sqlite:{path}")
    };
    let pool = SqlitePoolOptions::new()
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await?;
    Ok(pool)
}
