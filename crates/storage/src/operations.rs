//! Suspended sale and time-clock persistence.

use apex_edge_contracts::{ParkedCartSummary, TimeClockEntry};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::pool::PoolError;

pub struct ParkCartInput<'a> {
    pub cart_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub note: Option<&'a str>,
    pub cart_data: &'a Value,
    pub total_cents: u64,
    pub line_count: usize,
}

fn parse_datetime(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

pub async fn park_cart(
    pool: &SqlitePool,
    input: ParkCartInput<'_>,
) -> Result<ParkedCartSummary, PoolError> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO parked_carts (id, cart_id, store_id, register_id, note, cart_data, total_cents, line_count, parked_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(input.cart_id.to_string())
    .bind(input.store_id.to_string())
    .bind(input.register_id.to_string())
    .bind(input.note)
    .bind(input.cart_data.to_string())
    .bind(input.total_cents as i64)
    .bind(input.line_count as i64)
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;

    Ok(ParkedCartSummary {
        parked_cart_id: id,
        cart_id: input.cart_id,
        store_id: input.store_id,
        register_id: input.register_id,
        note: input.note.map(str::to_string),
        total_cents: input.total_cents,
        line_count: input.line_count,
        parked_at: now,
    })
}

pub async fn list_parked_carts(
    pool: &SqlitePool,
    store_id: Uuid,
    register_id: Option<Uuid>,
) -> Result<Vec<ParkedCartSummary>, PoolError> {
    let rows = if let Some(register_id) = register_id {
        sqlx::query(
            "SELECT * FROM parked_carts WHERE store_id = ? AND register_id = ? AND recalled_at IS NULL ORDER BY parked_at DESC",
        )
        .bind(store_id.to_string())
        .bind(register_id.to_string())
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT * FROM parked_carts WHERE store_id = ? AND recalled_at IS NULL ORDER BY parked_at DESC",
        )
        .bind(store_id.to_string())
        .fetch_all(pool)
        .await?
    };
    rows.into_iter().map(row_to_parked_cart).collect()
}

pub async fn recall_parked_cart(
    pool: &SqlitePool,
    parked_cart_id: Uuid,
) -> Result<Option<Value>, PoolError> {
    let Some(row) =
        sqlx::query("SELECT cart_data FROM parked_carts WHERE id = ? AND recalled_at IS NULL")
            .bind(parked_cart_id.to_string())
            .fetch_optional(pool)
            .await?
    else {
        return Ok(None);
    };
    sqlx::query("UPDATE parked_carts SET recalled_at = ? WHERE id = ?")
        .bind(Utc::now().to_rfc3339())
        .bind(parked_cart_id.to_string())
        .execute(pool)
        .await?;
    let data: String = row.try_get("cart_data")?;
    Ok(Some(serde_json::from_str(&data).unwrap_or(Value::Null)))
}

pub async fn clock_in(
    pool: &SqlitePool,
    store_id: Uuid,
    register_id: Uuid,
    associate_id: &str,
) -> Result<TimeClockEntry, PoolError> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO time_clock_entries (id, store_id, register_id, associate_id, clocked_in_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(register_id.to_string())
    .bind(associate_id)
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(TimeClockEntry {
        id,
        store_id,
        register_id,
        associate_id: associate_id.into(),
        clocked_in_at: now,
        clocked_out_at: None,
    })
}

pub async fn clock_out(
    pool: &SqlitePool,
    store_id: Uuid,
    associate_id: &str,
) -> Result<Option<TimeClockEntry>, PoolError> {
    let Some(row) = sqlx::query(
        "SELECT * FROM time_clock_entries WHERE store_id = ? AND associate_id = ? AND clocked_out_at IS NULL ORDER BY clocked_in_at DESC LIMIT 1",
    )
    .bind(store_id.to_string())
    .bind(associate_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let clocked_out_at = Utc::now();
    let id: String = row.try_get("id")?;
    sqlx::query("UPDATE time_clock_entries SET clocked_out_at = ? WHERE id = ?")
        .bind(clocked_out_at.to_rfc3339())
        .bind(&id)
        .execute(pool)
        .await?;
    Ok(Some(TimeClockEntry {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        store_id,
        register_id: Uuid::parse_str(&row.try_get::<String, _>("register_id")?).unwrap_or_default(),
        associate_id: associate_id.into(),
        clocked_in_at: parse_datetime(&row.try_get::<String, _>("clocked_in_at")?),
        clocked_out_at: Some(clocked_out_at),
    }))
}

fn row_to_parked_cart(row: sqlx::sqlite::SqliteRow) -> Result<ParkedCartSummary, PoolError> {
    let id: String = row.try_get("id")?;
    let cart_id: String = row.try_get("cart_id")?;
    let store_id: String = row.try_get("store_id")?;
    let register_id: String = row.try_get("register_id")?;
    let total_cents: i64 = row.try_get("total_cents")?;
    let line_count: i64 = row.try_get("line_count")?;
    let parked_at: String = row.try_get("parked_at")?;
    Ok(ParkedCartSummary {
        parked_cart_id: Uuid::parse_str(&id).unwrap_or_default(),
        cart_id: Uuid::parse_str(&cart_id).unwrap_or_default(),
        store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
        register_id: Uuid::parse_str(&register_id).unwrap_or_default(),
        note: row.try_get("note")?,
        total_cents: total_cents.max(0) as u64,
        line_count: line_count.max(0) as usize,
        parked_at: parse_datetime(&parked_at),
    })
}
