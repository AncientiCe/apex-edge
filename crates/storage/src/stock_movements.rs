//! Stock operation ledger.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

pub struct StockMovementInput<'a> {
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub item_id: Uuid,
    pub operation: &'a str,
    pub quantity_delta: i64,
    pub reason: &'a str,
    pub reference: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockMovement {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub item_id: Uuid,
    pub operation: String,
    pub quantity_delta: i64,
    pub reason: String,
    pub reference: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn insert_stock_movement(
    pool: &SqlitePool,
    input: StockMovementInput<'_>,
) -> Result<StockMovement, PoolError> {
    let movement = StockMovement {
        id: Uuid::new_v4(),
        store_id: input.store_id,
        register_id: input.register_id,
        item_id: input.item_id,
        operation: input.operation.into(),
        quantity_delta: input.quantity_delta,
        reason: input.reason.into(),
        reference: input.reference.map(str::to_string),
        created_at: Utc::now(),
    };
    sqlx::query(
        "INSERT INTO stock_movements (id, store_id, register_id, item_id, operation, quantity_delta, reason, reference, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(movement.id.to_string())
    .bind(input.store_id.to_string())
    .bind(input.register_id.to_string())
    .bind(input.item_id.to_string())
    .bind(input.operation)
    .bind(input.quantity_delta)
    .bind(input.reason)
    .bind(input.reference)
    .bind(movement.created_at.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(movement)
}
