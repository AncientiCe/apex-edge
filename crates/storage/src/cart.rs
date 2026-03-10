//! Cart persistence.

use apex_edge_contracts::CartStateKind;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug)]
pub struct CartRow {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub state: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

pub async fn save_cart(
    pool: &SqlitePool,
    id: Uuid,
    store_id: Uuid,
    register_id: Uuid,
    state: &CartStateKind,
    data: &Value,
) -> Result<(), PoolError> {
    let state_str = serde_json::to_string(state).unwrap_or_else(|_| "open".into());
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO carts (id, store_id, register_id, state, data, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET state = ?, data = ?, updated_at = ?
        "#,
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(register_id.to_string())
    .bind(&state_str)
    .bind(data.to_string())
    .bind(&now)
    .bind(&now)
    .bind(&state_str)
    .bind(data.to_string())
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn load_cart(pool: &SqlitePool, id: Uuid) -> Result<Option<CartRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, String, String)>(
        "SELECT id, store_id, register_id, state, data, created_at, updated_at FROM carts WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(id, store_id, register_id, state, data, created_at, updated_at)| CartRow {
            id: Uuid::parse_str(&id).unwrap_or_default(),
            store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
            register_id: Uuid::parse_str(&register_id).unwrap_or_default(),
            state,
            data: serde_json::from_str(&data).unwrap_or(Value::Null),
            created_at: parse_datetime(&created_at),
            updated_at: parse_datetime(&updated_at),
        },
    ))
}
