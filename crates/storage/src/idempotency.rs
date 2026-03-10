//! Idempotency keys for POS commands.

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

pub async fn get_response(pool: &SqlitePool, key: Uuid) -> Result<Option<String>, PoolError> {
    let row =
        sqlx::query_scalar::<_, Option<String>>("SELECT response FROM idempotency WHERE key = ?")
            .bind(key.to_string())
            .fetch_optional(pool)
            .await?;
    Ok(row.flatten())
}

pub async fn set_response(pool: &SqlitePool, key: Uuid, response: &str) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO idempotency (key, response, created_at) VALUES (?, ?, ?) \
         ON CONFLICT(key) DO UPDATE SET response = ?",
    )
    .bind(key.to_string())
    .bind(response)
    .bind(&now)
    .bind(response)
    .execute(pool)
    .await?;
    Ok(())
}
