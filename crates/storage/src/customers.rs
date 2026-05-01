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
    pub email: Option<String>,
}

fn map_customer_row(
    id: String,
    store_id: String,
    code: String,
    name: String,
    email: Option<String>,
) -> CustomerRow {
    CustomerRow {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
        code,
        name,
        email,
    }
}

pub async fn get_customer(
    pool: &SqlitePool,
    store_id: Uuid,
    customer_id: Uuid,
) -> Result<Option<CustomerRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String, Option<String>)>(
        "SELECT id, store_id, code, name, email FROM customers WHERE id = ? AND store_id = ?",
    )
    .bind(customer_id.to_string())
    .bind(store_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row
        .map(|(id, store_id, code, name, email)| map_customer_row(id, store_id, code, name, email)))
}

pub async fn get_customer_by_code(
    pool: &SqlitePool,
    store_id: Uuid,
    code: &str,
) -> Result<Option<CustomerRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String, Option<String>)>(
        "SELECT id, store_id, code, name, email FROM customers WHERE store_id = ? AND code = ?",
    )
    .bind(store_id.to_string())
    .bind(code)
    .fetch_optional(pool)
    .await?;
    Ok(row
        .map(|(id, store_id, code, name, email)| map_customer_row(id, store_id, code, name, email)))
}

pub async fn search_customers(
    pool: &SqlitePool,
    store_id: Uuid,
    q: &str,
) -> Result<Vec<CustomerRow>, PoolError> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let name_pattern = format!("%{q}%");
    let id_match = Uuid::parse_str(q).unwrap_or(Uuid::nil());
    let rows = sqlx::query_as::<_, (String, String, String, String, Option<String>)>(
        "SELECT id, store_id, code, name, email FROM customers WHERE store_id = ? \
         AND (code = ? OR name LIKE ? OR email LIKE ? OR id = ?) \
         ORDER BY name LIMIT 50",
    )
    .bind(store_id.to_string())
    .bind(q)
    .bind(&name_pattern)
    .bind(&name_pattern)
    .bind(id_match.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, store_id, code, name, email)| map_customer_row(id, store_id, code, name, email))
        .collect())
}

pub async fn insert_customer(
    pool: &SqlitePool,
    id: Uuid,
    store_id: Uuid,
    code: &str,
    name: &str,
    email: Option<&str>,
) -> Result<(), PoolError> {
    sqlx::query(
        "INSERT OR REPLACE INTO customers (id, store_id, code, name, email) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(code)
    .bind(name)
    .bind(email)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn pseudonymize_customer(
    pool: &SqlitePool,
    store_id: Uuid,
    customer_id: Uuid,
) -> Result<bool, PoolError> {
    let pseudonym = format!("erased-{}", customer_id);
    let result = sqlx::query(
        "UPDATE customers SET code = ?, name = 'Erased Customer', email = NULL WHERE id = ? AND store_id = ?",
    )
    .bind(pseudonym)
    .bind(customer_id.to_string())
    .bind(store_id.to_string())
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}
