//! Durable finalized order ledger.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewOrderLedgerEntry {
    pub order_id: Uuid,
    pub cart_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub shift_id: Option<Uuid>,
    pub subtotal_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
    pub total_cents: u64,
    pub submission_id: Option<Uuid>,
    pub lines: Vec<NewOrderLineEntry>,
    pub payments: Vec<NewOrderPaymentEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewOrderLineEntry {
    pub line_id: Uuid,
    pub item_id: Uuid,
    pub sku: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
    pub line_total_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewOrderPaymentEntry {
    pub tender_id: Uuid,
    pub tender_type: String,
    pub amount_cents: u64,
    pub external_reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderLedgerEntry {
    pub order_id: Uuid,
    pub cart_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub shift_id: Option<Uuid>,
    pub state: String,
    pub subtotal_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
    pub total_cents: u64,
    pub submission_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub finalized_at: DateTime<Utc>,
    pub lines: Vec<OrderLineEntry>,
    pub payments: Vec<OrderPaymentEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderLedgerSummary {
    pub order_id: Uuid,
    pub cart_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub shift_id: Option<Uuid>,
    pub state: String,
    pub total_cents: u64,
    pub finalized_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderLineEntry {
    pub line_id: Uuid,
    pub item_id: Uuid,
    pub sku: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
    pub line_total_cents: u64,
    pub discount_cents: u64,
    pub tax_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderPaymentEntry {
    pub payment_id: Uuid,
    pub tender_id: Uuid,
    pub tender_type: String,
    pub amount_cents: u64,
    pub external_reference: Option<String>,
}

fn parse_uuid(s: &str) -> Result<Uuid, PoolError> {
    Uuid::parse_str(s).map_err(|_| PoolError::Other("bad uuid".into()))
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn normalize_tender_type(tender_type: &str) -> String {
    let trimmed = tender_type.trim();
    if trimmed.is_empty() {
        "unknown".into()
    } else {
        trimmed.to_ascii_lowercase()
    }
}

pub async fn insert_order_ledger_entry(
    pool: &SqlitePool,
    order: &NewOrderLedgerEntry,
) -> Result<(), PoolError> {
    let mut tx = pool.begin().await?;
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO orders (id, cart_id, store_id, register_id, shift_id, state, subtotal_cents, discount_cents, tax_cents, total_cents, submission_id, created_at, finalized_at) \
         VALUES (?, ?, ?, ?, ?, 'finalized', ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET shift_id = excluded.shift_id, state = excluded.state, subtotal_cents = excluded.subtotal_cents, \
            discount_cents = excluded.discount_cents, tax_cents = excluded.tax_cents, total_cents = excluded.total_cents, submission_id = excluded.submission_id",
    )
    .bind(order.order_id.to_string())
    .bind(order.cart_id.to_string())
    .bind(order.store_id.to_string())
    .bind(order.register_id.to_string())
    .bind(order.shift_id.map(|id| id.to_string()))
    .bind(order.subtotal_cents as i64)
    .bind(order.discount_cents as i64)
    .bind(order.tax_cents as i64)
    .bind(order.total_cents as i64)
    .bind(order.submission_id.map(|id| id.to_string()))
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM order_lines WHERE order_id = ?")
        .bind(order.order_id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM order_payments WHERE order_id = ?")
        .bind(order.order_id.to_string())
        .execute(&mut *tx)
        .await?;

    for line in &order.lines {
        sqlx::query(
            "INSERT INTO order_lines (id, order_id, item_id, sku, name, quantity, unit_price_cents, line_total_cents, discount_cents, tax_cents, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(line.line_id.to_string())
        .bind(order.order_id.to_string())
        .bind(line.item_id.to_string())
        .bind(&line.sku)
        .bind(&line.name)
        .bind(line.quantity as i64)
        .bind(line.unit_price_cents as i64)
        .bind(line.line_total_cents as i64)
        .bind(line.discount_cents as i64)
        .bind(line.tax_cents as i64)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }

    for payment in &order.payments {
        sqlx::query(
            "INSERT INTO order_payments (id, order_id, tender_id, tender_type, amount_cents, external_reference, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(order.order_id.to_string())
        .bind(payment.tender_id.to_string())
        .bind(normalize_tender_type(&payment.tender_type))
        .bind(payment.amount_cents as i64)
        .bind(&payment.external_reference)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn fetch_order_ledger_entry(
    pool: &SqlitePool,
    order_id: Uuid,
) -> Result<Option<OrderLedgerEntry>, PoolError> {
    let Some(row) = sqlx::query("SELECT * FROM orders WHERE id = ?")
        .bind(order_id.to_string())
        .fetch_optional(pool)
        .await?
    else {
        return Ok(None);
    };
    let mut order = row_to_order(row)?;
    order.lines = list_order_lines(pool, order_id).await?;
    order.payments = list_order_payments(pool, order_id).await?;
    Ok(Some(order))
}

pub async fn list_order_ledger_entries(
    pool: &SqlitePool,
    store_id: Uuid,
    shift_id: Option<Uuid>,
) -> Result<Vec<OrderLedgerSummary>, PoolError> {
    let rows = if let Some(shift_id) = shift_id {
        sqlx::query(
            "SELECT id, cart_id, store_id, register_id, shift_id, state, total_cents, finalized_at \
             FROM orders WHERE store_id = ? AND shift_id = ? ORDER BY finalized_at DESC",
        )
        .bind(store_id.to_string())
        .bind(shift_id.to_string())
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT id, cart_id, store_id, register_id, shift_id, state, total_cents, finalized_at \
             FROM orders WHERE store_id = ? ORDER BY finalized_at DESC",
        )
        .bind(store_id.to_string())
        .fetch_all(pool)
        .await?
    };
    rows.into_iter().map(row_to_summary).collect()
}

pub async fn cash_sales_cents_for_shift(
    pool: &SqlitePool,
    shift_id: Uuid,
) -> Result<u64, PoolError> {
    let total: Option<i64> = sqlx::query_scalar(
        "SELECT SUM(p.amount_cents) FROM order_payments p \
         JOIN orders o ON o.id = p.order_id \
         WHERE o.shift_id = ? AND lower(p.tender_type) = 'cash'",
    )
    .bind(shift_id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(total.unwrap_or(0).max(0) as u64)
}

async fn list_order_lines(
    pool: &SqlitePool,
    order_id: Uuid,
) -> Result<Vec<OrderLineEntry>, PoolError> {
    let rows = sqlx::query("SELECT * FROM order_lines WHERE order_id = ? ORDER BY created_at ASC")
        .bind(order_id.to_string())
        .fetch_all(pool)
        .await?;
    rows.into_iter().map(row_to_line).collect()
}

async fn list_order_payments(
    pool: &SqlitePool,
    order_id: Uuid,
) -> Result<Vec<OrderPaymentEntry>, PoolError> {
    let rows =
        sqlx::query("SELECT * FROM order_payments WHERE order_id = ? ORDER BY created_at ASC")
            .bind(order_id.to_string())
            .fetch_all(pool)
            .await?;
    rows.into_iter().map(row_to_payment).collect()
}

fn row_to_order(row: sqlx::sqlite::SqliteRow) -> Result<OrderLedgerEntry, PoolError> {
    let id: String = row.try_get("id")?;
    let cart_id: String = row.try_get("cart_id")?;
    let store_id: String = row.try_get("store_id")?;
    let register_id: String = row.try_get("register_id")?;
    let shift_id: Option<String> = row.try_get("shift_id")?;
    let submission_id: Option<String> = row.try_get("submission_id")?;
    let subtotal: i64 = row.try_get("subtotal_cents")?;
    let discount: i64 = row.try_get("discount_cents")?;
    let tax: i64 = row.try_get("tax_cents")?;
    let total: i64 = row.try_get("total_cents")?;
    let created_at: String = row.try_get("created_at")?;
    let finalized_at: String = row.try_get("finalized_at")?;
    Ok(OrderLedgerEntry {
        order_id: parse_uuid(&id)?,
        cart_id: parse_uuid(&cart_id)?,
        store_id: parse_uuid(&store_id)?,
        register_id: parse_uuid(&register_id)?,
        shift_id: shift_id.as_deref().and_then(|id| Uuid::parse_str(id).ok()),
        state: row.try_get("state")?,
        subtotal_cents: subtotal.max(0) as u64,
        discount_cents: discount.max(0) as u64,
        tax_cents: tax.max(0) as u64,
        total_cents: total.max(0) as u64,
        submission_id: submission_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok()),
        created_at: parse_datetime(&created_at),
        finalized_at: parse_datetime(&finalized_at),
        lines: vec![],
        payments: vec![],
    })
}

fn row_to_summary(row: sqlx::sqlite::SqliteRow) -> Result<OrderLedgerSummary, PoolError> {
    let id: String = row.try_get("id")?;
    let cart_id: String = row.try_get("cart_id")?;
    let store_id: String = row.try_get("store_id")?;
    let register_id: String = row.try_get("register_id")?;
    let shift_id: Option<String> = row.try_get("shift_id")?;
    let total: i64 = row.try_get("total_cents")?;
    let finalized_at: String = row.try_get("finalized_at")?;
    Ok(OrderLedgerSummary {
        order_id: parse_uuid(&id)?,
        cart_id: parse_uuid(&cart_id)?,
        store_id: parse_uuid(&store_id)?,
        register_id: parse_uuid(&register_id)?,
        shift_id: shift_id.as_deref().and_then(|id| Uuid::parse_str(id).ok()),
        state: row.try_get("state")?,
        total_cents: total.max(0) as u64,
        finalized_at: parse_datetime(&finalized_at),
    })
}

fn row_to_line(row: sqlx::sqlite::SqliteRow) -> Result<OrderLineEntry, PoolError> {
    let id: String = row.try_get("id")?;
    let item_id: String = row.try_get("item_id")?;
    let quantity: i64 = row.try_get("quantity")?;
    let unit_price: i64 = row.try_get("unit_price_cents")?;
    let line_total: i64 = row.try_get("line_total_cents")?;
    let discount: i64 = row.try_get("discount_cents")?;
    let tax: i64 = row.try_get("tax_cents")?;
    Ok(OrderLineEntry {
        line_id: parse_uuid(&id)?,
        item_id: parse_uuid(&item_id)?,
        sku: row.try_get("sku")?,
        name: row.try_get("name")?,
        quantity: quantity.max(0) as u32,
        unit_price_cents: unit_price.max(0) as u64,
        line_total_cents: line_total.max(0) as u64,
        discount_cents: discount.max(0) as u64,
        tax_cents: tax.max(0) as u64,
    })
}

fn row_to_payment(row: sqlx::sqlite::SqliteRow) -> Result<OrderPaymentEntry, PoolError> {
    let id: String = row.try_get("id")?;
    let tender_id: String = row.try_get("tender_id")?;
    let amount: i64 = row.try_get("amount_cents")?;
    Ok(OrderPaymentEntry {
        payment_id: parse_uuid(&id)?,
        tender_id: parse_uuid(&tender_id)?,
        tender_type: row.try_get("tender_type")?,
        amount_cents: amount.max(0) as u64,
        external_reference: row.try_get("external_reference")?,
    })
}
