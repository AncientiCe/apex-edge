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
    /// True when `tax_cents` is contained in the price (VAT-inclusive).
    #[serde(default)]
    pub tax_inclusive: bool,
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

pub async fn finalize_return_row<'e, E>(executor: E, id: Uuid) -> Result<(), PoolError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE returns SET state = 'finalized', finalized_at = ? WHERE id = ?")
        .bind(&now)
        .bind(id.to_string())
        .execute(executor)
        .await?;
    Ok(())
}

/// Finalizes a return and queues it for HQ in one transaction, so a crash cannot leave a
/// refund that is final locally but never reported.
pub async fn finalize_return_row_with_outbox(
    pool: &SqlitePool,
    id: Uuid,
    outbox_id: Uuid,
    outbox_payload: &str,
) -> Result<(), PoolError> {
    let mut tx = pool.begin().await?;
    finalize_return_row(&mut *tx, id).await?;
    crate::outbox::insert_outbox(&mut *tx, outbox_id, outbox_payload).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn void_return_row(pool: &SqlitePool, id: Uuid) -> Result<(), PoolError> {
    let changed=sqlx::query("UPDATE returns SET state = 'voided' WHERE id = ? AND refunded_cents=0 AND NOT EXISTS (SELECT 1 FROM refund_intents WHERE return_id=returns.id)")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    if changed.rows_affected() != 1 {
        return Err(PoolError::Other(
            "a refunded or refund-pending return cannot be voided".into(),
        ));
    }
    Ok(())
}

pub async fn insert_return_line(pool: &SqlitePool, line: &ReturnLineRow) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO return_lines (id, return_id, original_line_id, sku, name, quantity, unit_price_cents, line_total_cents, tax_cents, tax_inclusive, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(line.tax_inclusive as i64)
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
        let tax_inclusive: i64 = r.try_get("tax_inclusive").unwrap_or(0);
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
            tax_inclusive: tax_inclusive != 0,
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

/// Reserve value before any external refund. A stable id binds retries to identical
/// terms and incomplete reservations count against the remaining return balance.
pub async fn prepare_refund(pool: &SqlitePool, refund: &RefundRow) -> Result<bool, PoolError> {
    let mut metric = ReturnWriteMetric::new("prepare_refund");
    let amount = i64::try_from(refund.amount_cents)
        .map_err(|_| PoolError::Other("invalid refund amount".into()))?;
    sqlx::query("INSERT INTO refund_intents (id, return_id, tender_type, amount_cents, created_at) \
        SELECT ?, r.id, ?, ?, ? FROM returns r WHERE r.id = ? AND r.state IN ('items','tendered') \
        AND ? > 0 AND r.total_cents >= ? + COALESCE((SELECT SUM(f.amount_cents) FROM refunds f WHERE f.return_id = r.id),0) \
        + COALESCE((SELECT SUM(i.amount_cents) FROM refund_intents i WHERE i.return_id=r.id AND NOT EXISTS (SELECT 1 FROM refunds f WHERE f.id=i.id)),0) \
        ON CONFLICT(id) DO NOTHING")
        .bind(refund.id.to_string()).bind(&refund.tender_type).bind(amount).bind(Utc::now().to_rfc3339())
        .bind(refund.return_id.to_string()).bind(amount).bind(amount).execute(pool).await?;
    let existing: Option<(String, String, i64)> =
        sqlx::query_as("SELECT return_id,tender_type,amount_cents FROM refund_intents WHERE id=?")
            .bind(refund.id.to_string())
            .fetch_optional(pool)
            .await?;
    match existing {
        Some((return_id, tender, stored))
            if return_id == refund.return_id.to_string()
                && tender == refund.tender_type
                && stored == amount => {}
        _ => {
            return Err(PoolError::Other(
                "refund exceeds remaining balance or command key was reused with different terms"
                    .into(),
            ))
        }
    }
    let recorded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM refunds WHERE id=?")
        .bind(refund.id.to_string())
        .fetch_one(pool)
        .await?;
    metric.success = true;
    Ok(recorded > 0)
}

/// Commit the provider result and return totals together. Repeating this after a
/// crash is harmless; the primary key is the durable refund identity.
pub async fn complete_refund(pool: &SqlitePool, refund: &RefundRow) -> Result<(), PoolError> {
    let mut metric = ReturnWriteMetric::new("complete_refund");
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO refunds (id,return_id,tender_type,amount_cents,external_reference,created_at) VALUES (?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING")
        .bind(refund.id.to_string()).bind(refund.return_id.to_string()).bind(&refund.tender_type)
        .bind(refund.amount_cents as i64).bind(&refund.external_reference).bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
    sqlx::query("UPDATE returns SET refunded_cents=(SELECT COALESCE(SUM(amount_cents),0) FROM refunds WHERE return_id=returns.id), \
        state=CASE WHEN (SELECT COALESCE(SUM(amount_cents),0) FROM refunds WHERE return_id=returns.id)>=total_cents THEN 'paid' ELSE 'tendered' END WHERE id=?")
        .bind(refund.return_id.to_string()).execute(&mut *tx).await?;
    tx.commit().await?;
    metric.success = true;
    Ok(())
}

/// Reserve original-sale quantities across all non-voided returns and price from
/// the ledger. The first write acquires SQLite's writer lock before any reads.
pub async fn add_receipted_return_line(
    pool: &SqlitePool,
    input: &ReturnLineRow,
) -> Result<(), PoolError> {
    let mut metric = ReturnWriteMetric::new("add_receipted_return_line");
    if input.quantity == 0 {
        return Err(PoolError::Other("quantity must be positive".into()));
    }
    let mut tx = pool.begin().await?;
    let locked =
        sqlx::query("UPDATE returns SET state=state WHERE id=? AND state IN ('open','items')")
            .bind(input.return_id.to_string())
            .execute(&mut *tx)
            .await?;
    if locked.rows_affected() != 1 {
        return Err(PoolError::Other("return cannot accept items".into()));
    }
    let candidates=sqlx::query("SELECT l.*, o.discount_cents AS order_discount FROM order_lines l JOIN orders o ON o.id=l.order_id \
        JOIN returns r ON r.original_order_id=o.id AND r.store_id=o.store_id WHERE r.id=? AND (l.id=? OR (? IS NULL AND l.sku=?))")
        .bind(input.return_id.to_string()).bind(input.original_line_id.map(|id|id.to_string()))
        .bind(input.original_line_id.map(|id|id.to_string())).bind(&input.sku).fetch_all(&mut *tx).await?;
    if candidates.len() != 1 {
        return Err(PoolError::Other(
            "original order line is missing or ambiguous".into(),
        ));
    }
    let original = &candidates[0];
    let original_id: String = original.try_get("id")?;
    let sku: String = original.try_get("sku")?;
    if sku != input.sku {
        return Err(PoolError::Other("SKU does not match original line".into()));
    }
    let sold: i64 = original.try_get("quantity")?;
    let (used, paid_used, tax_used):(i64,i64,i64)=sqlx::query_as("SELECT COALESCE(SUM(l.quantity),0),COALESCE(SUM(l.line_total_cents),0),COALESCE(SUM(l.tax_cents),0) FROM return_lines l JOIN returns r ON r.id=l.return_id WHERE l.original_line_id=? AND (r.state!='voided' OR r.refunded_cents>0 OR EXISTS (SELECT 1 FROM refund_intents i WHERE i.return_id=r.id))")
        .bind(&original_id).fetch_one(&mut *tx).await?;
    let next = used
        .checked_add(i64::from(input.quantity))
        .ok_or_else(|| PoolError::Other("quantity overflow".into()))?;
    if sold <= 0 || used < 0 || next > sold {
        return Err(PoolError::Other(
            "quantity exceeds original purchase less previous returns".into(),
        ));
    }
    let order_id: String = original.try_get("order_id")?;
    let totals:(i64,i64)=sqlx::query_as("SELECT COALESCE(SUM(line_total_cents-discount_cents),0),COALESCE(SUM(discount_cents),0) FROM order_lines WHERE order_id=?")
        .bind(&order_id).fetch_one(&mut *tx).await?;
    let prefix:i64=sqlx::query_scalar("SELECT COALESCE(SUM(line_total_cents-discount_cents),0) FROM order_lines WHERE order_id=? AND id<?")
        .bind(&order_id).bind(&original_id).fetch_one(&mut *tx).await?;
    let line_total: i64 = original.try_get("line_total_cents")?;
    let discount: i64 = original.try_get("discount_cents")?;
    let base = line_total.saturating_sub(discount).max(0);
    let extra = original
        .try_get::<i64, _>("order_discount")?
        .saturating_sub(totals.1)
        .max(0)
        .min(totals.0.max(0));
    let proportion = |total: i64, qty: i64, denom: i64| -> i64 {
        if denom <= 0 {
            0
        } else {
            (i128::from(total) * i128::from(qty) / i128::from(denom)) as i64
        }
    };
    let basket_share =
        proportion(extra, prefix + base, totals.0) - proportion(extra, prefix, totals.0);
    let paid = base.saturating_sub(basket_share).max(0);
    let tax: i64 = original.try_get("tax_cents")?;
    let amount = proportion(paid, next, sold) - paid_used;
    let tax_amount = proportion(tax, next, sold) - tax_used;
    if amount < 0 || tax_amount < 0 {
        return Err(PoolError::Other(
            "original return values require reconciliation".into(),
        ));
    }
    let inclusive: i64 = original.try_get("tax_inclusive")?;
    let name: String = original.try_get("name")?;
    sqlx::query("INSERT INTO return_lines (id,return_id,original_line_id,sku,name,quantity,unit_price_cents,line_total_cents,tax_cents,tax_inclusive,created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
        .bind(input.id.to_string()).bind(input.return_id.to_string()).bind(original_id).bind(sku).bind(name).bind(i64::from(input.quantity))
        .bind(amount/i64::from(input.quantity)).bind(amount).bind(tax_amount).bind(inclusive).bind(Utc::now().to_rfc3339()).execute(&mut *tx).await?;
    sqlx::query("UPDATE returns SET total_cents=(SELECT COALESCE(SUM(line_total_cents+CASE WHEN tax_inclusive=0 THEN tax_cents ELSE 0 END),0) FROM return_lines WHERE return_id=returns.id), \
        tax_cents=(SELECT COALESCE(SUM(tax_cents),0) FROM return_lines WHERE return_id=returns.id),state='items' WHERE id=?")
        .bind(input.return_id.to_string()).execute(&mut *tx).await?;
    tx.commit().await?;
    metric.success = true;
    Ok(())
}

struct ReturnWriteMetric {
    operation: &'static str,
    started: std::time::Instant,
    success: bool,
}
impl ReturnWriteMetric {
    fn new(operation: &'static str) -> Self {
        Self {
            operation,
            started: std::time::Instant::now(),
            success: false,
        }
    }
}
impl Drop for ReturnWriteMetric {
    fn drop(&mut self) {
        metrics::counter!(apex_edge_metrics::DB_OPERATIONS_TOTAL, "operation"=>self.operation, "outcome"=>if self.success {"success"} else {"error"}).increment(1);
        metrics::histogram!(apex_edge_metrics::DB_OPERATION_DURATION_SECONDS, "operation"=>self.operation).record(self.started.elapsed().as_secs_f64());
    }
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
                tax_inclusive: false,
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
