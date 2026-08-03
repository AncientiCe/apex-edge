//! Gift card storage: local-first balance tracking, activation, and redemption.
//!
//! Reuses `apex_edge_giftcards::GiftCardState` for the card lifecycle, but balance and
//! state mutations are guarded atomic SQL `UPDATE ... WHERE` statements rather than a
//! read-modify-write through the domain state machine, so two registers redeeming or
//! reloading the same physical card concurrently cannot race past a stale in-memory
//! balance. This mirrors the oversell guard used by the real-time inventory ledger
//! (`crates/storage/src/inventory_ledger.rs`).

use apex_edge_giftcards::GiftCardState;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GiftCardRecord {
    pub id: Uuid,
    pub store_id: Uuid,
    pub code: String,
    pub balance_cents: u64,
    pub currency: String,
    pub state: GiftCardState,
    pub updated_at: DateTime<Utc>,
}

fn state_to_str(state: &GiftCardState) -> &'static str {
    match state {
        GiftCardState::Issued => "issued",
        GiftCardState::Active => "active",
        GiftCardState::Disabled => "disabled",
    }
}

fn state_from_str(s: &str) -> GiftCardState {
    match s {
        "active" => GiftCardState::Active,
        "disabled" => GiftCardState::Disabled,
        _ => GiftCardState::Issued,
    }
}

fn row_to_record(row: SqliteRow) -> Result<GiftCardRecord, PoolError> {
    let id: String = row.try_get("id")?;
    let store_id: String = row.try_get("store_id")?;
    let code: String = row.try_get("code")?;
    let balance_cents: i64 = row.try_get("balance_cents")?;
    let currency: String = row.try_get("currency")?;
    let state: String = row.try_get("state")?;
    let updated_at: String = row.try_get("updated_at")?;
    Ok(GiftCardRecord {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
        code,
        balance_cents: balance_cents.max(0) as u64,
        currency,
        state: state_from_str(&state),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}

pub async fn get_gift_card_by_code(
    pool: &SqlitePool,
    code: &str,
) -> Result<Option<GiftCardRecord>, PoolError> {
    let row = sqlx::query(
        "SELECT id, store_id, code, balance_cents, currency, state, updated_at \
         FROM gift_cards WHERE code = ?",
    )
    .bind(code)
    .fetch_optional(pool)
    .await?;
    row.map(row_to_record).transpose()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueOutcome {
    Issued,
    DuplicateCode,
}

/// Issue a new gift card in the `Issued` (not yet loaded/usable) state. Callers pick the
/// code (e.g. a generated one) up front so duplicate detection is a single insert attempt
/// against the table's `UNIQUE(code)` constraint rather than a check-then-insert race.
pub async fn issue_gift_card(
    pool: &SqlitePool,
    store_id: Uuid,
    code: &str,
    currency: &str,
) -> Result<(IssueOutcome, Option<GiftCardRecord>), PoolError> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let res = sqlx::query(
        "INSERT INTO gift_cards (id, store_id, code, balance_cents, currency, state, updated_at) \
         VALUES (?, ?, ?, 0, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(code)
    .bind(currency)
    .bind(state_to_str(&GiftCardState::Issued))
    .bind(now.to_rfc3339())
    .execute(pool)
    .await;
    match res {
        Ok(_) => Ok((
            IssueOutcome::Issued,
            Some(GiftCardRecord {
                id,
                store_id,
                code: code.to_string(),
                balance_cents: 0,
                currency: currency.to_string(),
                state: GiftCardState::Issued,
                updated_at: now,
            }),
        )),
        Err(e) => {
            let is_duplicate = e
                .as_database_error()
                .map(|d| d.is_unique_violation())
                .unwrap_or(false);
            if is_duplicate {
                Ok((IssueOutcome::DuplicateCode, None))
            } else {
                Err(e.into())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivateOutcome {
    Activated,
    NotFound,
    AlreadyActivated,
    InvalidAmount,
}

/// Activate an issued card with its opening balance. Guarded on `state = 'issued'` so a
/// card cannot be activated twice (e.g. two registers scanning the same fresh card).
pub async fn activate_gift_card(
    pool: &SqlitePool,
    code: &str,
    opening_balance_cents: u64,
) -> Result<(ActivateOutcome, Option<GiftCardRecord>), PoolError> {
    if opening_balance_cents == 0 {
        return Ok((ActivateOutcome::InvalidAmount, None));
    }
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "UPDATE gift_cards SET balance_cents = ?, state = 'active', updated_at = ? \
         WHERE code = ? AND state = 'issued'",
    )
    .bind(opening_balance_cents as i64)
    .bind(&now)
    .bind(code)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return match get_gift_card_by_code(pool, code).await? {
            None => Ok((ActivateOutcome::NotFound, None)),
            Some(existing) => Ok((ActivateOutcome::AlreadyActivated, Some(existing))),
        };
    }
    Ok((
        ActivateOutcome::Activated,
        get_gift_card_by_code(pool, code).await?,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadOutcome {
    Reloaded,
    NotFound,
    NotActive,
    InvalidAmount,
}

/// Add funds to an already-active card. Guarded on `state = 'active'`.
pub async fn reload_gift_card(
    pool: &SqlitePool,
    code: &str,
    amount_cents: u64,
) -> Result<(ReloadOutcome, Option<GiftCardRecord>), PoolError> {
    if amount_cents == 0 {
        return Ok((ReloadOutcome::InvalidAmount, None));
    }
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "UPDATE gift_cards SET balance_cents = balance_cents + ?, updated_at = ? \
         WHERE code = ? AND state = 'active'",
    )
    .bind(amount_cents as i64)
    .bind(&now)
    .bind(code)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return match get_gift_card_by_code(pool, code).await? {
            None => Ok((ReloadOutcome::NotFound, None)),
            Some(existing) => Ok((ReloadOutcome::NotActive, Some(existing))),
        };
    }
    Ok((
        ReloadOutcome::Reloaded,
        get_gift_card_by_code(pool, code).await?,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedeemOutcome {
    Redeemed,
    NotFound,
    NotActive,
    InsufficientBalance,
    InvalidAmount,
}

/// Debit an active card. The `balance_cents >= ?` guard in the `WHERE` clause makes this
/// a single atomic check-and-decrement: concurrent redemptions against the same card can
/// never drive the balance negative, no matter how they interleave.
pub async fn redeem_gift_card(
    pool: &SqlitePool,
    code: &str,
    amount_cents: u64,
) -> Result<(RedeemOutcome, Option<GiftCardRecord>), PoolError> {
    if amount_cents == 0 {
        return Ok((RedeemOutcome::InvalidAmount, None));
    }
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "UPDATE gift_cards SET balance_cents = balance_cents - ?, updated_at = ? \
         WHERE code = ? AND state = 'active' AND balance_cents >= ?",
    )
    .bind(amount_cents as i64)
    .bind(&now)
    .bind(code)
    .bind(amount_cents as i64)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return match get_gift_card_by_code(pool, code).await? {
            None => Ok((RedeemOutcome::NotFound, None)),
            Some(existing) if existing.state != GiftCardState::Active => {
                Ok((RedeemOutcome::NotActive, Some(existing)))
            }
            Some(existing) => Ok((RedeemOutcome::InsufficientBalance, Some(existing))),
        };
    }
    Ok((
        RedeemOutcome::Redeemed,
        get_gift_card_by_code(pool, code).await?,
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
    async fn issue_then_lookup_returns_issued_card_with_zero_balance() {
        let pool = test_pool().await;
        let store_id = Uuid::new_v4();
        let (outcome, record) = issue_gift_card(&pool, store_id, "GC-001", "USD")
            .await
            .unwrap();
        assert_eq!(outcome, IssueOutcome::Issued);
        let record = record.unwrap();
        assert_eq!(record.state, GiftCardState::Issued);
        assert_eq!(record.balance_cents, 0);

        let fetched = get_gift_card_by_code(&pool, "GC-001")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched, record);
    }

    #[tokio::test]
    async fn issuing_duplicate_code_is_rejected() {
        let pool = test_pool().await;
        let store_id = Uuid::new_v4();
        issue_gift_card(&pool, store_id, "GC-DUP", "USD")
            .await
            .unwrap();
        let (outcome, record) = issue_gift_card(&pool, store_id, "GC-DUP", "USD")
            .await
            .unwrap();
        assert_eq!(outcome, IssueOutcome::DuplicateCode);
        assert!(record.is_none());
    }

    #[tokio::test]
    async fn activate_sets_opening_balance_and_active_state() {
        let pool = test_pool().await;
        issue_gift_card(&pool, Uuid::new_v4(), "GC-ACT", "USD")
            .await
            .unwrap();
        let (outcome, record) = activate_gift_card(&pool, "GC-ACT", 2_500).await.unwrap();
        assert_eq!(outcome, ActivateOutcome::Activated);
        let record = record.unwrap();
        assert_eq!(record.state, GiftCardState::Active);
        assert_eq!(record.balance_cents, 2_500);
    }

    #[tokio::test]
    async fn activating_twice_is_rejected() {
        let pool = test_pool().await;
        issue_gift_card(&pool, Uuid::new_v4(), "GC-TWICE", "USD")
            .await
            .unwrap();
        activate_gift_card(&pool, "GC-TWICE", 1_000).await.unwrap();
        let (outcome, _) = activate_gift_card(&pool, "GC-TWICE", 500).await.unwrap();
        assert_eq!(outcome, ActivateOutcome::AlreadyActivated);
    }

    #[tokio::test]
    async fn activating_unknown_code_reports_not_found() {
        let pool = test_pool().await;
        let (outcome, record) = activate_gift_card(&pool, "GC-NOPE", 500).await.unwrap();
        assert_eq!(outcome, ActivateOutcome::NotFound);
        assert!(record.is_none());
    }

    #[tokio::test]
    async fn activating_with_zero_balance_is_rejected() {
        let pool = test_pool().await;
        issue_gift_card(&pool, Uuid::new_v4(), "GC-ZERO", "USD")
            .await
            .unwrap();
        let (outcome, _) = activate_gift_card(&pool, "GC-ZERO", 0).await.unwrap();
        assert_eq!(outcome, ActivateOutcome::InvalidAmount);
    }

    #[tokio::test]
    async fn reload_adds_to_balance_of_active_card() {
        let pool = test_pool().await;
        issue_gift_card(&pool, Uuid::new_v4(), "GC-RELOAD", "USD")
            .await
            .unwrap();
        activate_gift_card(&pool, "GC-RELOAD", 1_000).await.unwrap();
        let (outcome, record) = reload_gift_card(&pool, "GC-RELOAD", 500).await.unwrap();
        assert_eq!(outcome, ReloadOutcome::Reloaded);
        assert_eq!(record.unwrap().balance_cents, 1_500);
    }

    #[tokio::test]
    async fn reload_before_activation_is_rejected() {
        let pool = test_pool().await;
        issue_gift_card(&pool, Uuid::new_v4(), "GC-NOTACTIVE", "USD")
            .await
            .unwrap();
        let (outcome, _) = reload_gift_card(&pool, "GC-NOTACTIVE", 500).await.unwrap();
        assert_eq!(outcome, ReloadOutcome::NotActive);
    }

    #[tokio::test]
    async fn redeem_deducts_balance_when_sufficient() {
        let pool = test_pool().await;
        issue_gift_card(&pool, Uuid::new_v4(), "GC-REDEEM", "USD")
            .await
            .unwrap();
        activate_gift_card(&pool, "GC-REDEEM", 1_000).await.unwrap();
        let (outcome, record) = redeem_gift_card(&pool, "GC-REDEEM", 400).await.unwrap();
        assert_eq!(outcome, RedeemOutcome::Redeemed);
        assert_eq!(record.unwrap().balance_cents, 600);
    }

    #[tokio::test]
    async fn redeem_rejects_amount_over_balance() {
        let pool = test_pool().await;
        issue_gift_card(&pool, Uuid::new_v4(), "GC-OVER", "USD")
            .await
            .unwrap();
        activate_gift_card(&pool, "GC-OVER", 500).await.unwrap();
        let (outcome, record) = redeem_gift_card(&pool, "GC-OVER", 600).await.unwrap();
        assert_eq!(outcome, RedeemOutcome::InsufficientBalance);
        // Balance must be unchanged after a rejected redemption.
        assert_eq!(record.unwrap().balance_cents, 500);
    }

    #[tokio::test]
    async fn redeem_rejects_disabled_or_unactivated_card() {
        let pool = test_pool().await;
        issue_gift_card(&pool, Uuid::new_v4(), "GC-INACTIVE", "USD")
            .await
            .unwrap();
        let (outcome, _) = redeem_gift_card(&pool, "GC-INACTIVE", 100).await.unwrap();
        assert_eq!(outcome, RedeemOutcome::NotActive);
    }

    #[tokio::test]
    async fn concurrent_redemptions_never_overdraw_balance() {
        let pool = test_pool().await;
        issue_gift_card(&pool, Uuid::new_v4(), "GC-RACE", "USD")
            .await
            .unwrap();
        activate_gift_card(&pool, "GC-RACE", 1_000).await.unwrap();

        let mut handles = vec![];
        for _ in 0..10 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                redeem_gift_card(&pool, "GC-RACE", 200).await.unwrap().0
            }));
        }
        let mut redeemed = 0;
        for h in handles {
            if h.await.unwrap() == RedeemOutcome::Redeemed {
                redeemed += 1;
            }
        }
        // 1_000 / 200 = exactly 5 can succeed; the rest must be rejected, never overdrawn.
        assert_eq!(redeemed, 5);
        let final_balance = get_gift_card_by_code(&pool, "GC-RACE")
            .await
            .unwrap()
            .unwrap()
            .balance_cents;
        assert_eq!(final_balance, 0);
    }
}
