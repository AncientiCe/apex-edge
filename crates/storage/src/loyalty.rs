//! Loyalty points storage: local-first points balance tracking.
//!
//! Earning is unconditionally additive (`points = points + delta`, upserting the account
//! on first earn). Redeeming is a guarded atomic `UPDATE ... WHERE points >= ?`, the same
//! oversell-guard pattern used by the gift card and inventory ledger stores, so concurrent
//! redemptions against the same customer account can never drive the balance negative.

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoyaltyAccountRecord {
    pub customer_id: Uuid,
    pub points: u64,
    pub updated_at: DateTime<Utc>,
}

fn row_to_record(row: SqliteRow) -> Result<LoyaltyAccountRecord, PoolError> {
    let customer_id: String = row.try_get("customer_id")?;
    let points: i64 = row.try_get("points")?;
    let updated_at: String = row.try_get("updated_at")?;
    Ok(LoyaltyAccountRecord {
        customer_id: Uuid::parse_str(&customer_id).unwrap_or_default(),
        points: points.max(0) as u64,
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}

pub async fn get_loyalty_account(
    pool: &SqlitePool,
    customer_id: Uuid,
) -> Result<Option<LoyaltyAccountRecord>, PoolError> {
    let row = sqlx::query(
        "SELECT customer_id, points, updated_at FROM loyalty_accounts WHERE customer_id = ?",
    )
    .bind(customer_id.to_string())
    .fetch_optional(pool)
    .await?;
    row.map(row_to_record).transpose()
}

/// Add points to a customer's account, creating the account (starting at 0) on first earn.
pub async fn earn_loyalty_points(
    pool: &SqlitePool,
    customer_id: Uuid,
    points: u64,
) -> Result<LoyaltyAccountRecord, PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO loyalty_accounts (customer_id, points, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(customer_id) DO UPDATE SET points = points + excluded.points, updated_at = excluded.updated_at",
    )
    .bind(customer_id.to_string())
    .bind(points as i64)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(get_loyalty_account(pool, customer_id)
        .await?
        .unwrap_or(LoyaltyAccountRecord {
            customer_id,
            points,
            updated_at: Utc::now(),
        }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedeemPointsOutcome {
    Redeemed,
    NotFound,
    InsufficientPoints,
    InvalidAmount,
}

/// Debit points from a customer's account. The `points >= ?` guard in the `WHERE` clause
/// makes this a single atomic check-and-decrement, exactly like `redeem_gift_card`.
pub async fn redeem_loyalty_points(
    pool: &SqlitePool,
    customer_id: Uuid,
    points: u64,
) -> Result<(RedeemPointsOutcome, Option<LoyaltyAccountRecord>), PoolError> {
    if points == 0 {
        return Ok((RedeemPointsOutcome::InvalidAmount, None));
    }
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "UPDATE loyalty_accounts SET points = points - ?, updated_at = ? \
         WHERE customer_id = ? AND points >= ?",
    )
    .bind(points as i64)
    .bind(&now)
    .bind(customer_id.to_string())
    .bind(points as i64)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return match get_loyalty_account(pool, customer_id).await? {
            None => Ok((RedeemPointsOutcome::NotFound, None)),
            Some(existing) => Ok((RedeemPointsOutcome::InsufficientPoints, Some(existing))),
        };
    }
    Ok((
        RedeemPointsOutcome::Redeemed,
        get_loyalty_account(pool, customer_id).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_migrations;
    use crate::pool::create_sqlite_pool;

    async fn test_pool() -> SqlitePool {
        let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn earning_creates_account_on_first_earn() {
        let pool = test_pool().await;
        let customer_id = Uuid::new_v4();
        let record = earn_loyalty_points(&pool, customer_id, 12).await.unwrap();
        assert_eq!(record.points, 12);

        let fetched = get_loyalty_account(&pool, customer_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched, record);
    }

    #[tokio::test]
    async fn earning_twice_accumulates_points() {
        let pool = test_pool().await;
        let customer_id = Uuid::new_v4();
        earn_loyalty_points(&pool, customer_id, 10).await.unwrap();
        let record = earn_loyalty_points(&pool, customer_id, 5).await.unwrap();
        assert_eq!(record.points, 15);
    }

    #[tokio::test]
    async fn redeem_deducts_points_when_sufficient() {
        let pool = test_pool().await;
        let customer_id = Uuid::new_v4();
        earn_loyalty_points(&pool, customer_id, 20).await.unwrap();
        let (outcome, record) = redeem_loyalty_points(&pool, customer_id, 8).await.unwrap();
        assert_eq!(outcome, RedeemPointsOutcome::Redeemed);
        assert_eq!(record.unwrap().points, 12);
    }

    #[tokio::test]
    async fn redeem_rejects_amount_over_balance() {
        let pool = test_pool().await;
        let customer_id = Uuid::new_v4();
        earn_loyalty_points(&pool, customer_id, 5).await.unwrap();
        let (outcome, record) = redeem_loyalty_points(&pool, customer_id, 6).await.unwrap();
        assert_eq!(outcome, RedeemPointsOutcome::InsufficientPoints);
        assert_eq!(record.unwrap().points, 5);
    }

    #[tokio::test]
    async fn redeem_against_unknown_customer_reports_not_found() {
        let pool = test_pool().await;
        let (outcome, record) = redeem_loyalty_points(&pool, Uuid::new_v4(), 5)
            .await
            .unwrap();
        assert_eq!(outcome, RedeemPointsOutcome::NotFound);
        assert!(record.is_none());
    }

    #[tokio::test]
    async fn redeem_zero_points_is_rejected() {
        let pool = test_pool().await;
        let customer_id = Uuid::new_v4();
        earn_loyalty_points(&pool, customer_id, 5).await.unwrap();
        let (outcome, _) = redeem_loyalty_points(&pool, customer_id, 0).await.unwrap();
        assert_eq!(outcome, RedeemPointsOutcome::InvalidAmount);
    }

    #[tokio::test]
    async fn concurrent_redemptions_never_overdraw_points() {
        let pool = test_pool().await;
        let customer_id = Uuid::new_v4();
        earn_loyalty_points(&pool, customer_id, 100).await.unwrap();

        let mut handles = vec![];
        for _ in 0..10 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                redeem_loyalty_points(&pool, customer_id, 20)
                    .await
                    .unwrap()
                    .0
            }));
        }
        let mut redeemed = 0;
        for h in handles {
            if h.await.unwrap() == RedeemPointsOutcome::Redeemed {
                redeemed += 1;
            }
        }
        assert_eq!(redeemed, 5);
        let final_points = get_loyalty_account(&pool, customer_id)
            .await
            .unwrap()
            .unwrap()
            .points;
        assert_eq!(final_points, 0);
    }
}
