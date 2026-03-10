//! Customers (for search and cart association).

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone)]
pub struct CustomerRow {
    pub id: Uuid,
    pub store_id: Uuid,
    pub code: String,
    pub name: String,
}

pub async fn get_customer(
    pool: &SqlitePool,
    store_id: Uuid,
    customer_id: Uuid,
) -> Result<Option<CustomerRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT id, store_id, code, name FROM customers WHERE id = ? AND store_id = ?",
    )
    .bind(customer_id.to_string())
    .bind(store_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, store_id, code, name)| CustomerRow {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
        code,
        name,
    }))
}

pub async fn get_customer_by_code(
    pool: &SqlitePool,
    store_id: Uuid,
    code: &str,
) -> Result<Option<CustomerRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT id, store_id, code, name FROM customers WHERE store_id = ? AND code = ?",
    )
    .bind(store_id.to_string())
    .bind(code)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id, store_id, code, name)| CustomerRow {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
        code,
        name,
    }))
}

pub async fn insert_customer(
    pool: &SqlitePool,
    id: Uuid,
    store_id: Uuid,
    code: &str,
    name: &str,
) -> Result<(), PoolError> {
    sqlx::query(
        "INSERT OR REPLACE INTO customers (id, store_id, code, name) VALUES (?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(code)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(())
}
