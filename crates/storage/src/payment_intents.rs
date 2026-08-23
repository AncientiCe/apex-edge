//! Payment intent ledger.
//!
//! Every provider-side payment gets a durable row before and after the money moves, so
//! that a hub which crashes mid-checkout can still tell the difference between "the
//! customer paid and owns a sale" and "the customer paid for nothing". The latter is
//! recoverable only if it was written down.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentIntentState {
    /// Funds held, not yet taken.
    Authorized,
    /// Funds taken; the order is not yet known to be durable.
    Captured,
    /// Funds taken and the order is on the ledger. Terminal, and never reversed.
    Settled,
    /// Capture succeeded but the sale did not. Owes a void.
    ReversalPending,
    /// Void or refund confirmed by the provider.
    Reversed,
    /// The provider said no. Nothing to reverse.
    Declined,
    /// The provider could not be reached and no funds moved.
    Failed,
}

impl PaymentIntentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PaymentIntentState::Authorized => "authorized",
            PaymentIntentState::Captured => "captured",
            PaymentIntentState::Settled => "settled",
            PaymentIntentState::ReversalPending => "reversal_pending",
            PaymentIntentState::Reversed => "reversed",
            PaymentIntentState::Declined => "declined",
            PaymentIntentState::Failed => "failed",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "captured" => PaymentIntentState::Captured,
            "settled" => PaymentIntentState::Settled,
            "reversal_pending" => PaymentIntentState::ReversalPending,
            "reversed" => PaymentIntentState::Reversed,
            "declined" => PaymentIntentState::Declined,
            "failed" => PaymentIntentState::Failed,
            _ => PaymentIntentState::Authorized,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewPaymentIntent {
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub cart_id: Uuid,
    pub tender_id: Uuid,
    pub idempotency_key: Uuid,
    pub provider: String,
    pub amount_cents: u64,
    pub tip_amount_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentIntentRecord {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub cart_id: Uuid,
    pub tender_id: Uuid,
    pub provider: String,
    pub provider_payment_id: Option<String>,
    pub amount_cents: u64,
    pub tip_amount_cents: u64,
    pub approved_cents: u64,
    pub state: PaymentIntentState,
    pub failure_code: Option<String>,
    pub order_id: Option<Uuid>,
    pub reversal_attempts: i64,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const SELECT_COLUMNS: &str = "id, store_id, register_id, cart_id, tender_id, provider, \
     provider_payment_id, amount_cents, tip_amount_cents, approved_cents, state, failure_code, \
     order_id, reversal_attempts, last_error, created_at, updated_at";

fn parse_uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap_or_else(|_| Uuid::nil())
}

fn parse_timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn row_to_record(row: &sqlx::sqlite::SqliteRow) -> PaymentIntentRecord {
    let amount_cents: i64 = row.try_get("amount_cents").unwrap_or_default();
    let tip_amount_cents: i64 = row.try_get("tip_amount_cents").unwrap_or_default();
    let approved_cents: i64 = row.try_get("approved_cents").unwrap_or_default();
    let state: String = row.try_get("state").unwrap_or_default();
    let order_id: Option<String> = row.try_get("order_id").unwrap_or_default();
    let created_at: String = row.try_get("created_at").unwrap_or_default();
    let updated_at: String = row.try_get("updated_at").unwrap_or_default();

    PaymentIntentRecord {
        id: parse_uuid(&row.try_get::<String, _>("id").unwrap_or_default()),
        store_id: parse_uuid(&row.try_get::<String, _>("store_id").unwrap_or_default()),
        register_id: parse_uuid(&row.try_get::<String, _>("register_id").unwrap_or_default()),
        cart_id: parse_uuid(&row.try_get::<String, _>("cart_id").unwrap_or_default()),
        tender_id: parse_uuid(&row.try_get::<String, _>("tender_id").unwrap_or_default()),
        provider: row.try_get("provider").unwrap_or_default(),
        provider_payment_id: row.try_get("provider_payment_id").unwrap_or_default(),
        amount_cents: amount_cents.max(0) as u64,
        tip_amount_cents: tip_amount_cents.max(0) as u64,
        approved_cents: approved_cents.max(0) as u64,
        state: PaymentIntentState::from_str(&state),
        failure_code: row.try_get("failure_code").unwrap_or_default(),
        order_id: order_id.as_deref().map(parse_uuid),
        reversal_attempts: row.try_get("reversal_attempts").unwrap_or_default(),
        last_error: row.try_get("last_error").unwrap_or_default(),
        created_at: parse_timestamp(&created_at),
        updated_at: parse_timestamp(&updated_at),
    }
}

/// Record a new authorization, or return the existing one when the POS retried the same
/// command. The uniqueness of (idempotency_key, provider, tender_id) is what stops a
/// retry from becoming a second charge.
pub async fn insert_payment_intent(
    pool: &SqlitePool,
    input: NewPaymentIntent,
) -> Result<PaymentIntentRecord, PoolError> {
    if let Some(existing) = sqlx::query(&format!(
        "SELECT {SELECT_COLUMNS} FROM payment_intents \
         WHERE idempotency_key = ? AND provider = ? AND tender_id = ?"
    ))
    .bind(input.idempotency_key.to_string())
    .bind(&input.provider)
    .bind(input.tender_id.to_string())
    .fetch_optional(pool)
    .await?
    {
        return Ok(row_to_record(&existing));
    }

    let now = Utc::now();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payment_intents (id, store_id, register_id, cart_id, tender_id, \
         idempotency_key, provider, amount_cents, tip_amount_cents, approved_cents, state, \
         reversal_attempts, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, 0, ?, ?)",
    )
    .bind(id.to_string())
    .bind(input.store_id.to_string())
    .bind(input.register_id.to_string())
    .bind(input.cart_id.to_string())
    .bind(input.tender_id.to_string())
    .bind(input.idempotency_key.to_string())
    .bind(&input.provider)
    .bind(input.amount_cents as i64)
    .bind(input.tip_amount_cents as i64)
    .bind(PaymentIntentState::Authorized.as_str())
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;

    Ok(PaymentIntentRecord {
        id,
        store_id: input.store_id,
        register_id: input.register_id,
        cart_id: input.cart_id,
        tender_id: input.tender_id,
        provider: input.provider,
        provider_payment_id: None,
        amount_cents: input.amount_cents,
        tip_amount_cents: input.tip_amount_cents,
        approved_cents: 0,
        state: PaymentIntentState::Authorized,
        failure_code: None,
        order_id: None,
        reversal_attempts: 0,
        last_error: None,
        created_at: now,
        updated_at: now,
    })
}

pub async fn get_payment_intent(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<Option<PaymentIntentRecord>, PoolError> {
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLUMNS} FROM payment_intents WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_record))
}

pub async fn list_payment_intents_for_cart(
    pool: &SqlitePool,
    store_id: Uuid,
    cart_id: Uuid,
) -> Result<Vec<PaymentIntentRecord>, PoolError> {
    let rows = sqlx::query(&format!(
        "SELECT {SELECT_COLUMNS} FROM payment_intents \
         WHERE store_id = ? AND cart_id = ? ORDER BY created_at"
    ))
    .bind(store_id.to_string())
    .bind(cart_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_record).collect())
}

pub async fn mark_payment_intent_captured(
    pool: &SqlitePool,
    id: Uuid,
    provider_payment_id: &str,
    approved_cents: u64,
) -> Result<(), PoolError> {
    sqlx::query(
        "UPDATE payment_intents SET state = ?, provider_payment_id = ?, approved_cents = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(PaymentIntentState::Captured.as_str())
    .bind(provider_payment_id)
    .bind(approved_cents as i64)
    .bind(Utc::now().to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_payment_intent_declined(
    pool: &SqlitePool,
    id: Uuid,
    failure_code: &str,
) -> Result<(), PoolError> {
    sqlx::query(
        "UPDATE payment_intents SET state = ?, failure_code = ?, updated_at = ? WHERE id = ?",
    )
    .bind(PaymentIntentState::Declined.as_str())
    .bind(failure_code)
    .bind(Utc::now().to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// Record that the provider could not be reached and no funds moved.
pub async fn mark_payment_intent_failed(
    pool: &SqlitePool,
    id: Uuid,
    error: &str,
) -> Result<(), PoolError> {
    sqlx::query(
        "UPDATE payment_intents SET state = ?, last_error = ?, updated_at = ? WHERE id = ?",
    )
    .bind(PaymentIntentState::Failed.as_str())
    .bind(error)
    .bind(Utc::now().to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// The order is durable, so every captured payment for this cart is now safe.
/// Settled is terminal: the reversal sweeper must never touch these rows again.
pub async fn settle_payment_intents_for_cart(
    pool: &SqlitePool,
    store_id: Uuid,
    cart_id: Uuid,
    order_id: Uuid,
) -> Result<u64, PoolError> {
    let result = sqlx::query(
        "UPDATE payment_intents SET state = ?, order_id = ?, updated_at = ? \
         WHERE store_id = ? AND cart_id = ? AND state IN (?, ?)",
    )
    .bind(PaymentIntentState::Settled.as_str())
    .bind(order_id.to_string())
    .bind(Utc::now().to_rfc3339())
    .bind(store_id.to_string())
    .bind(cart_id.to_string())
    .bind(PaymentIntentState::Captured.as_str())
    .bind(PaymentIntentState::Authorized.as_str())
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// The sale did not complete, so any money already taken for this cart is owed back.
/// Only captured payments are flagged: settled ones belong to a real order, and
/// authorized ones were never charged.
pub async fn flag_payment_intents_for_reversal(
    pool: &SqlitePool,
    store_id: Uuid,
    cart_id: Uuid,
    reason: &str,
) -> Result<u64, PoolError> {
    let result = sqlx::query(
        "UPDATE payment_intents SET state = ?, last_error = ?, updated_at = ? \
         WHERE store_id = ? AND cart_id = ? AND state = ?",
    )
    .bind(PaymentIntentState::ReversalPending.as_str())
    .bind(reason)
    .bind(Utc::now().to_rfc3339())
    .bind(store_id.to_string())
    .bind(cart_id.to_string())
    .bind(PaymentIntentState::Captured.as_str())
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Flag a single intent, for the case where the provider gave an indeterminate answer
/// and we do not yet know whether it took the money.
pub async fn flag_payment_intent_for_reversal(
    pool: &SqlitePool,
    id: Uuid,
    provider_payment_id: Option<&str>,
    reason: &str,
) -> Result<(), PoolError> {
    sqlx::query(
        "UPDATE payment_intents SET state = ?, last_error = ?, \
         provider_payment_id = COALESCE(?, provider_payment_id), updated_at = ? WHERE id = ?",
    )
    .bind(PaymentIntentState::ReversalPending.as_str())
    .bind(reason)
    .bind(provider_payment_id)
    .bind(Utc::now().to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// Flag captured payments that have been sitting without an order for longer than
/// `stale_after_seconds`.
///
/// This covers the case a graceful failure path cannot: the process died between
/// capturing a card and doing anything about it, so no code ever ran to flag the row.
/// Anything still `captured` well after a checkout should have completed is money the
/// store is holding for nothing.
pub async fn flag_stale_captured_payment_intents(
    pool: &SqlitePool,
    stale_after_seconds: i64,
) -> Result<u64, PoolError> {
    let cutoff = Utc::now() - chrono::Duration::seconds(stale_after_seconds);
    let result = sqlx::query(
        "UPDATE payment_intents SET state = ?, last_error = ?, updated_at = ? \
         WHERE state = ? AND order_id IS NULL AND updated_at < ?",
    )
    .bind(PaymentIntentState::ReversalPending.as_str())
    .bind("captured payment was never claimed by a completed sale")
    .bind(Utc::now().to_rfc3339())
    .bind(PaymentIntentState::Captured.as_str())
    .bind(cutoff.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn fetch_payment_intents_awaiting_reversal(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<PaymentIntentRecord>, PoolError> {
    let rows = sqlx::query(&format!(
        "SELECT {SELECT_COLUMNS} FROM payment_intents WHERE state = ? \
         ORDER BY updated_at LIMIT ?"
    ))
    .bind(PaymentIntentState::ReversalPending.as_str())
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_record).collect())
}

pub async fn mark_payment_intent_reversed(pool: &SqlitePool, id: Uuid) -> Result<(), PoolError> {
    sqlx::query("UPDATE payment_intents SET state = ?, updated_at = ? WHERE id = ?")
        .bind(PaymentIntentState::Reversed.as_str())
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

/// A reversal attempt failed. The row stays in the queue: unreversed money owed to a
/// customer is never dropped just because the provider was unreachable.
pub async fn record_payment_intent_reversal_failure(
    pool: &SqlitePool,
    id: Uuid,
    error: &str,
) -> Result<(), PoolError> {
    sqlx::query(
        "UPDATE payment_intents SET reversal_attempts = reversal_attempts + 1, \
         last_error = ?, updated_at = ? WHERE id = ?",
    )
    .bind(error)
    .bind(Utc::now().to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_through_its_storage_form() {
        for state in [
            PaymentIntentState::Authorized,
            PaymentIntentState::Captured,
            PaymentIntentState::Settled,
            PaymentIntentState::ReversalPending,
            PaymentIntentState::Reversed,
            PaymentIntentState::Declined,
            PaymentIntentState::Failed,
        ] {
            assert_eq!(PaymentIntentState::from_str(state.as_str()), state);
        }
    }

    #[test]
    fn an_unknown_state_reads_back_as_authorized_not_settled() {
        // Failing safe matters here: mistaking an unknown row for "settled" would hide
        // money owed to a customer.
        assert_eq!(
            PaymentIntentState::from_str("something_new"),
            PaymentIntentState::Authorized
        );
    }
}
