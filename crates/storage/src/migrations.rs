//! Schema migrations.

use sqlx::SqlitePool;
use thiserror::Error;

const MIGRATION_001: &str = include_str!("../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../migrations/002_catalog_pricing.sql");
const MIGRATION_003: &str = include_str!("../migrations/003_categories_and_search.sql");
const MIGRATION_004: &str = include_str!("../migrations/004_sync_status.sql");
const MIGRATION_006: &str = include_str!("../migrations/006_print_templates.sql");
const MIGRATION_007: &str = include_str!("../migrations/007_auth.sql");
const MIGRATION_009: &str = include_str!("../migrations/009_coupon_definitions.sql");

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

async fn column_exists(
    pool: &SqlitePool,
    table: &str,
    column: &str,
) -> Result<bool, MigrationError> {
    let row =
        sqlx::query_as::<_, (String,)>("SELECT name FROM pragma_table_info(?) WHERE name = ?")
            .bind(table)
            .bind(column)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<(), MigrationError> {
    for sql in &[MIGRATION_001, MIGRATION_002, MIGRATION_003, MIGRATION_004] {
        let sql_no_comments = strip_sql_comment_lines(sql);
        for stmt in sql_no_comments.split(';').filter(|s| !s.trim().is_empty()) {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            sqlx::query(stmt).execute(pool).await?;
        }
    }
    if !column_exists(pool, "customers", "email").await? {
        if let Err(e) = sqlx::query("ALTER TABLE customers ADD COLUMN email TEXT")
            .execute(pool)
            .await
        {
            if !e.to_string().contains("duplicate column name") {
                return Err(e.into());
            }
        }
    }
    if !column_exists(pool, "catalog_items", "description").await? {
        if let Err(e) = sqlx::query("ALTER TABLE catalog_items ADD COLUMN description TEXT")
            .execute(pool)
            .await
        {
            if !e.to_string().contains("duplicate column name") {
                return Err(e.into());
            }
        }
    }
    // Migration 005: inventory availability fields (idempotent ADD COLUMN).
    for (table, column, ddl) in &[
        (
            "catalog_items",
            "is_active",
            "ALTER TABLE catalog_items ADD COLUMN is_active INTEGER NOT NULL DEFAULT 1",
        ),
        (
            "catalog_items",
            "available_qty",
            "ALTER TABLE catalog_items ADD COLUMN available_qty INTEGER",
        ),
        (
            "catalog_items",
            "is_available",
            "ALTER TABLE catalog_items ADD COLUMN is_available INTEGER",
        ),
        (
            "catalog_items",
            "image_urls",
            "ALTER TABLE catalog_items ADD COLUMN image_urls TEXT NOT NULL DEFAULT '[]'",
        ),
        (
            "catalog_items",
            "raw_product_json",
            "ALTER TABLE catalog_items ADD COLUMN raw_product_json TEXT",
        ),
    ] {
        if !column_exists(pool, table, column).await? {
            if let Err(e) = sqlx::query(ddl).execute(pool).await {
                if !e.to_string().contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }
    }
    for sql in &[MIGRATION_006, MIGRATION_007, MIGRATION_009] {
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
