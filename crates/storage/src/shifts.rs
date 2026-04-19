//! Till & Shift persistence.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShiftRow {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub associate_id: Option<String>,
    pub state: String,
    pub opening_float_cents: u64,
    pub closing_counted_cents: Option<i64>,
    pub expected_cents: Option<i64>,
    pub variance_cents: Option<i64>,
    pub opened_at: String,
    pub closed_at: Option<String>,
}

pub async fn open_shift(
    pool: &SqlitePool,
    id: Uuid,
    store_id: Uuid,
    register_id: Uuid,
    associate_id: Option<&str>,
    opening_float_cents: u64,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO shifts (id, store_id, register_id, associate_id, state, opening_float_cents, opened_at) \
         VALUES (?, ?, ?, ?, 'open', ?, ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(register_id.to_string())
    .bind(associate_id)
    .bind(opening_float_cents as i64)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn fetch_open_shift(
    pool: &SqlitePool,
    store_id: Uuid,
    register_id: Uuid,
) -> Result<Option<ShiftRow>, PoolError> {
    let row = sqlx::query(
        "SELECT * FROM shifts WHERE store_id = ? AND register_id = ? AND state = 'open'",
    )
    .bind(store_id.to_string())
    .bind(register_id.to_string())
    .fetch_optional(pool)
    .await?;
    match row {
        Some(r) => Ok(Some(row_to_shift(r)?)),
        None => Ok(None),
    }
}

pub async fn fetch_shift(pool: &SqlitePool, id: Uuid) -> Result<Option<ShiftRow>, PoolError> {
    let row = sqlx::query("SELECT * FROM shifts WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    match row {
        Some(r) => Ok(Some(row_to_shift(r)?)),
        None => Ok(None),
    }
}

fn row_to_shift(r: sqlx::sqlite::SqliteRow) -> Result<ShiftRow, PoolError> {
    let id_s: String = r.try_get("id")?;
    let store_s: String = r.try_get("store_id")?;
    let reg_s: String = r.try_get("register_id")?;
    let opening: i64 = r.try_get("opening_float_cents")?;
    Ok(ShiftRow {
        id: Uuid::parse_str(&id_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
        store_id: Uuid::parse_str(&store_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
        register_id: Uuid::parse_str(&reg_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
        associate_id: r.try_get("associate_id")?,
        state: r.try_get("state")?,
        opening_float_cents: opening.max(0) as u64,
        closing_counted_cents: r.try_get("closing_counted_cents")?,
        expected_cents: r.try_get("expected_cents")?,
        variance_cents: r.try_get("variance_cents")?,
        opened_at: r.try_get("opened_at")?,
        closed_at: r.try_get("closed_at")?,
    })
}

pub async fn close_shift(
    pool: &SqlitePool,
    id: Uuid,
    counted_cents: i64,
    expected_cents: i64,
    variance_cents: i64,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE shifts SET state = 'closed', closing_counted_cents = ?, expected_cents = ?, \
            variance_cents = ?, closed_at = ? WHERE id = ?",
    )
    .bind(counted_cents)
    .bind(expected_cents)
    .bind(variance_cents)
    .bind(&now)
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_shift_movement(
    pool: &SqlitePool,
    id: Uuid,
    shift_id: Uuid,
    kind: &str,
    amount_cents: u64,
    reason: Option<&str>,
    approval_id: Option<Uuid>,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO shift_movements (id, shift_id, kind, amount_cents, reason, approval_id, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(shift_id.to_string())
    .bind(kind)
    .bind(amount_cents as i64)
    .bind(reason)
    .bind(approval_id.map(|u| u.to_string()))
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShiftMovementRow {
    pub id: Uuid,
    pub shift_id: Uuid,
    pub kind: String,
    pub amount_cents: u64,
    pub reason: Option<String>,
    pub approval_id: Option<Uuid>,
}

pub async fn list_shift_movements(
    pool: &SqlitePool,
    shift_id: Uuid,
) -> Result<Vec<ShiftMovementRow>, PoolError> {
    let rows =
        sqlx::query("SELECT * FROM shift_movements WHERE shift_id = ? ORDER BY created_at ASC")
            .bind(shift_id.to_string())
            .fetch_all(pool)
            .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id_s: String = r.try_get("id")?;
        let sid_s: String = r.try_get("shift_id")?;
        let approval_s: Option<String> = r.try_get("approval_id")?;
        let amt: i64 = r.try_get("amount_cents")?;
        out.push(ShiftMovementRow {
            id: Uuid::parse_str(&id_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
            shift_id: Uuid::parse_str(&sid_s).map_err(|_| PoolError::Other("bad uuid".into()))?,
            kind: r.try_get("kind")?,
            amount_cents: amt.max(0) as u64,
            reason: r.try_get("reason")?,
            approval_id: approval_s.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
        });
    }
    Ok(out)
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
    async fn one_open_shift_per_register() {
        let pool = setup().await;
        let store = Uuid::new_v4();
        let reg = Uuid::new_v4();
        open_shift(&pool, Uuid::new_v4(), store, reg, Some("assoc-1"), 10_000)
            .await
            .unwrap();

        let err = open_shift(&pool, Uuid::new_v4(), store, reg, Some("assoc-2"), 20_000).await;
        assert!(err.is_err(), "second open shift must fail on same register");
    }

    #[tokio::test]
    async fn movements_round_trip() {
        let pool = setup().await;
        let sid = Uuid::new_v4();
        open_shift(&pool, sid, Uuid::nil(), Uuid::nil(), None, 5_000)
            .await
            .unwrap();
        insert_shift_movement(
            &pool,
            Uuid::new_v4(),
            sid,
            "paid_in",
            100,
            Some("tip"),
            None,
        )
        .await
        .unwrap();
        insert_shift_movement(&pool, Uuid::new_v4(), sid, "paid_out", 50, None, None)
            .await
            .unwrap();
        let ms = list_shift_movements(&pool, sid).await.unwrap();
        assert_eq!(ms.len(), 2);
    }

    #[tokio::test]
    async fn close_stores_variance_and_state() {
        let pool = setup().await;
        let sid = Uuid::new_v4();
        open_shift(&pool, sid, Uuid::nil(), Uuid::nil(), None, 10_000)
            .await
            .unwrap();
        close_shift(&pool, sid, 10_200, 10_000, 200).await.unwrap();
        let row = fetch_shift(&pool, sid).await.unwrap().unwrap();
        assert_eq!(row.state, "closed");
        assert_eq!(row.variance_cents, Some(200));
    }
}
