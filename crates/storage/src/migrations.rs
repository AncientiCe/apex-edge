//! Schema migrations.

use sqlx::SqlitePool;
use thiserror::Error;

const MIGRATION_001: &str = include_str!("../migrations/001_init.sql");

#[derive(Error, Debug)]
pub enum MigrationError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<(), MigrationError> {
    for stmt in MIGRATION_001.split(';').filter(|s| !s.trim().is_empty()) {
        let stmt = stmt.trim();
        if stmt.is_empty() || stmt.starts_with("--") {
            continue;
        }
        sqlx::query(stmt).execute(pool).await?;
    }
    Ok(())
}
