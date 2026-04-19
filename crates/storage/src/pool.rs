//! DB pool (SQLite for edge, optional Postgres).

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use std::time::Duration;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PoolError {
    #[error("connection: {0}")]
    Connection(#[from] sqlx::Error),
    #[error("{0}")]
    Other(String),
}

/// Create a SQLite pool from either:
/// - a SQLx URL (e.g. `sqlite::memory:`), or
/// - a plain path (e.g. `./apex_edge.db`, `/data/apex_edge.db`).
///
/// # Examples
///
/// ```no_run
/// use apex_edge_storage::create_sqlite_pool;
///
/// let _memory_pool = create_sqlite_pool("sqlite::memory:");
/// let _file_pool = create_sqlite_pool("./apex_edge.db");
/// ```
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
        // Absolute Windows path: use forward slashes so the URL is valid (sqlite:///C:/path/to/file.db)
        let path_forward = path.replace('\\', "/");
        format!("sqlite:///{path_forward}")
    } else {
        // Relative path
        format!("sqlite:{path}")
    };
    // For file-based DBs, allow creating the file if missing (mode=rwc). In-memory URLs are unchanged.
    let url_with_mode = if url.contains(":memory:") {
        url
    } else if url.contains('?') {
        format!("{url}&mode=rwc")
    } else {
        format!("{url}?mode=rwc")
    };
    let pool = SqlitePoolOptions::new()
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url_with_mode)
        .await?;
    Ok(pool)
}
