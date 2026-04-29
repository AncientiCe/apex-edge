//! Returns & Refunds persistence.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewReturn {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub shift_id: Option<Uuid>,
    pub original_order_id: Option<Uuid>,
    pub reason_code: Option<String>,
    pub approval_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnRow {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub shift_id: Option<Uuid>,
    pub original_order_id: Option<Uuid>,
    pub reason_code: Option<String>,
    pub state: String,
    pub total_cents: u64,
    pub tax_cents: u64,
    pub refunded_cents: u64,
    pub approval_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnLineRow {
    pub id: Uuid,
    pub return_id: Uuid,
    pub original_line_id: Option<Uuid>,
    pub sku: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
    pub line_total_cents: u64,
    pub tax_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefundRow {
    pub id: Uuid,
    pub return_id: Uuid,
    pub tender_type: String,
    pub amount_cents: u64,
    pub external_reference: Option<String>,
}

pub async fn insert_return(pool: &SqlitePool, r: &NewReturn) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO returns (id, store_id, register_id, shift_id, original_order_id, reason_code, state, approval_id, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'open', ?, ?)",
    )
    .bind(r.id.to_string())
    .bind(r.store_id.to_string())
    .bind(r.register_id.to_string())
    .bind(r.shift_id.map(|u| u.to_string()))
    .bind(r.original_order_id.map(|u| u.to_string()))
    .bind(&r.reason_code)
    .bind(r.approval_id.map(|u| u.to_string()))
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn fetch_return(pool: &SqlitePool, id: Uuid) -> Result<Option<ReturnRow>, PoolError> {
    let row = sqlx::query("SELECT * FROM returns WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    match row {
        Some(r) => {
            let id_s: String = r.try_get("id")?;
            let store_s: String = r.try_get("store_id")?;
            let reg_s: String = r.try_get("register_id")?;
            let shift_s: Option<String> = r.try_get("shift_id")?;
            let ooi_s: Option<String> = r.try_get("original_order_id")?;
            let approval_s: Option<String> = r.try_get("approval_id")?;
            let state: String = r.try_get("state")?;
            let total: i64 = r.try_get("total_cents")?;
            let tax: i64 = r.try_get("tax_cents")?;
            let refunded: i64 = r.try_get("refunded_cents")?;
            Ok(Some(ReturnRow {
                id: Uuid::parse_str(&id_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
                store_id: Uuid::parse_str(&store_s)
                    .map_err(|_| PoolError::Other("bad uuid".into()))?,
                register_id: Uuid::parse_str(&reg_s)
                    .map_err(|_| PoolError::Other("bad uuid".into()))?,
                shift_id: shift_s.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
                original_order_id: ooi_s.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
                reason_code: r.try_get("reason_code")?,
                state,
                total_cents: total.max(0) as u64,
                tax_cents: tax.max(0) as u64,
                refunded_cents: refunded.max(0) as u64,
                approval_id: approval_s.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
            }))
        }
        None => Ok(None),
    }
}

pub async fn update_return_totals(
    pool: &SqlitePool,
    id: Uuid,
    total_cents: u64,
    tax_cents: u64,
    refunded_cents: u64,
    state: &str,
) -> Result<(), PoolError> {
    sqlx::query(
        "UPDATE returns SET total_cents = ?, tax_cents = ?, refunded_cents = ?, state = ? WHERE id = ?",
    )
    .bind(total_cents as i64)
    .bind(tax_cents as i64)
    .bind(refunded_cents as i64)
    .bind(state)
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn finalize_return_row(pool: &SqlitePool, id: Uuid) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE returns SET state = 'finalized', finalized_at = ? WHERE id = ?")
        .bind(&now)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn void_return_row(pool: &SqlitePool, id: Uuid) -> Result<(), PoolError> {
    sqlx::query("UPDATE returns SET state = 'voided' WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn insert_return_line(pool: &SqlitePool, line: &ReturnLineRow) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO return_lines (id, return_id, original_line_id, sku, name, quantity, unit_price_cents, line_total_cents, tax_cents, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(line.id.to_string())
    .bind(line.return_id.to_string())
    .bind(line.original_line_id.map(|u| u.to_string()))
    .bind(&line.sku)
    .bind(&line.name)
    .bind(line.quantity as i64)
    .bind(line.unit_price_cents as i64)
    .bind(line.line_total_cents as i64)
    .bind(line.tax_cents as i64)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_return_lines(
    pool: &SqlitePool,
    return_id: Uuid,
) -> Result<Vec<ReturnLineRow>, PoolError> {
    let rows =
        sqlx::query("SELECT * FROM return_lines WHERE return_id = ? ORDER BY created_at ASC")
            .bind(return_id.to_string())
            .fetch_all(pool)
            .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id_s: String = r.try_get("id")?;
        let ret_s: String = r.try_get("return_id")?;
        let orig_s: Option<String> = r.try_get("original_line_id")?;
        let qty: i64 = r.try_get("quantity")?;
        let price: i64 = r.try_get("unit_price_cents")?;
        let total: i64 = r.try_get("line_total_cents")?;
        let tax: i64 = r.try_get("tax_cents")?;
        out.push(ReturnLineRow {
            id: Uuid::parse_str(&id_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
            return_id: Uuid::parse_str(&ret_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
            original_line_id: orig_s.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
            sku: r.try_get("sku")?,
            name: r.try_get("name")?,
            quantity: qty.max(0) as u32,
            unit_price_cents: price.max(0) as u64,
            line_total_cents: total.max(0) as u64,
            tax_cents: tax.max(0) as u64,
        });
    }
    Ok(out)
}

pub async fn insert_refund(pool: &SqlitePool, refund: &RefundRow) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO refunds (id, return_id, tender_type, amount_cents, external_reference, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(refund.id.to_string())
    .bind(refund.return_id.to_string())
    .bind(&refund.tender_type)
    .bind(refund.amount_cents as i64)
    .bind(&refund.external_reference)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_refunds(pool: &SqlitePool, return_id: Uuid) -> Result<Vec<RefundRow>, PoolError> {
    let rows = sqlx::query("SELECT * FROM refunds WHERE return_id = ? ORDER BY created_at ASC")
        .bind(return_id.to_string())
        .fetch_all(pool)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id_s: String = r.try_get("id")?;
        let ret_s: String = r.try_get("return_id")?;
        let amount: i64 = r.try_get("amount_cents")?;
        out.push(RefundRow {
            id: Uuid::parse_str(&id_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
            return_id: Uuid::parse_str(&ret_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
            tender_type: r.try_get("tender_type")?,
            amount_cents: amount.max(0) as u64,
            external_reference: r.try_get("external_reference")?,
        });
    }
    Ok(out)
}

pub async fn cash_refunds_cents_for_shift(
    pool: &SqlitePool,
    shift_id: Uuid,
) -> Result<u64, PoolError> {
    let total: Option<i64> = sqlx::query_scalar(
        "SELECT SUM(f.amount_cents) FROM refunds f \
         JOIN returns r ON r.id = f.return_id \
         WHERE r.shift_id = ? AND r.state = 'finalized' AND lower(f.tender_type) = 'cash'",
    )
    .bind(shift_id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(total.unwrap_or(0).max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_sqlite_pool, run_migrations};

    async fn setup() -> SqlitePool {
        let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn insert_and_fetch_return() {
        let pool = setup().await;
        let id = Uuid::new_v4();
        insert_return(
            &pool,
            &NewReturn {
                id,
                store_id: Uuid::nil(),
                register_id: Uuid::new_v4(),
                shift_id: None,
                original_order_id: Some(Uuid::new_v4()),
                reason_code: Some("damaged".into()),
                approval_id: None,
            },
        )
        .await
        .unwrap();

        let row = fetch_return(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.state, "open");
        assert_eq!(row.total_cents, 0);
    }

    #[tokio::test]
    async fn lines_and_refunds_roundtrip() {
        let pool = setup().await;
        let rid = Uuid::new_v4();
        insert_return(
            &pool,
            &NewReturn {
                id: rid,
                store_id: Uuid::nil(),
                register_id: Uuid::nil(),
                shift_id: None,
                original_order_id: None,
                reason_code: None,
                approval_id: Some(Uuid::new_v4()),
            },
        )
        .await
        .unwrap();

        insert_return_line(
            &pool,
            &ReturnLineRow {
                id: Uuid::new_v4(),
                return_id: rid,
                original_line_id: None,
                sku: "SKU-1".into(),
                name: "Widget".into(),
                quantity: 2,
                unit_price_cents: 500,
                line_total_cents: 1000,
                tax_cents: 100,
            },
        )
        .await
        .unwrap();

        insert_refund(
            &pool,
            &RefundRow {
                id: Uuid::new_v4(),
                return_id: rid,
                tender_type: "cash".into(),
                amount_cents: 1100,
                external_reference: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(list_return_lines(&pool, rid).await.unwrap().len(), 1);
        assert_eq!(list_refunds(&pool, rid).await.unwrap().len(), 1);
    }
}
