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
    // Strip SQL comment-only lines first so comment headers don't hide real
    // statements when splitting by ';' (e.g. "-- Carts\nCREATE TABLE ...;").
    let sql_no_comments = MIGRATION_001
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");

    for stmt in sql_no_comments.split(';').filter(|s| !s.trim().is_empty()) {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        sqlx::query(stmt).execute(pool).await?;
    }
    Ok(())
}
