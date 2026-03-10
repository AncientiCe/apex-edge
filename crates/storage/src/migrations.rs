//! Schema migrations.

use sqlx::SqlitePool;
use thiserror::Error;

const MIGRATION_001: &str = include_str!("../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../migrations/002_catalog_pricing.sql");

fn strip_sql_comment_lines(sql: &str) -> String {
    sql.lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Error, Debug)]
pub enum MigrationError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<(), MigrationError> {
    for sql in &[MIGRATION_001, MIGRATION_002] {
        let sql_no_comments = strip_sql_comment_lines(sql);
        for stmt in sql_no_comments.split(';').filter(|s| !s.trim().is_empty()) {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            sqlx::query(stmt).execute(pool).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_sql_comment_lines;

    #[test]
    fn comment_lines_are_removed_but_sql_is_kept() {
        let sql = "-- header\nCREATE TABLE t(id INTEGER);\n-- footer";
        let out = strip_sql_comment_lines(sql);
        assert!(out.contains("CREATE TABLE t"));
        assert!(!out.contains("--"));
    }
}
