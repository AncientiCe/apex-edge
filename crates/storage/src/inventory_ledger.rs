//! Real-time inventory ledger storage.
//!
//! HQ remains authoritative for on-hand stock (`hq_baseline_qty`). The edge layers
//! real-time `reserved_qty`, `sold_since_sync_qty`, and `local_adjust_qty` on top so
//! that concurrent registers cannot oversell between periodic HQ syncs.
//!
//! `available_to_sell = max(0, hq_baseline + local_adjust - reserved - sold_since_sync)`.
//!
//! Items with no `inventory_state` row are *untracked* (no stock constraint), matching
//! the legacy behaviour where `catalog_items.available_qty = NULL` never blocks a sale.

use apex_edge_contracts::InventoryLevel;
use apex_edge_domain::availability::InventoryLedgerState;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

/// Outcome summary of reconciling a batch of HQ inventory levels into the ledger.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileSummary {
    /// Number of item levels rebased.
    pub items: u64,
    /// Number of items where local activity exceeded the fresh HQ baseline (clamped to 0).
    pub drift: u64,
}

/// Snapshot of the four ledger quantities for one item, with the derived availability.
#[derive(Debug, Clone)]
pub struct InventoryStateRow {
    pub store_id: Uuid,
    pub item_id: Uuid,
    pub hq_baseline_qty: i64,
    pub reserved_qty: i64,
    pub sold_since_sync_qty: i64,
    pub local_adjust_qty: i64,
    pub baseline_as_of: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl InventoryStateRow {
    pub fn ledger_state(&self) -> InventoryLedgerState {
        InventoryLedgerState {
            hq_baseline_qty: self.hq_baseline_qty,
            reserved_qty: self.reserved_qty,
            sold_since_sync_qty: self.sold_since_sync_qty,
            local_adjust_qty: self.local_adjust_qty,
        }
    }

    pub fn available_to_sell(&self) -> i64 {
        self.ledger_state().available_to_sell()
    }
}

/// Outcome of a reservation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveOutcome {
    /// Item is tracked and the units were reserved.
    Reserved,
    /// Item has no ledger row (untracked) — no stock constraint applied.
    Untracked,
    /// Item is tracked but cannot satisfy the request. Carries current availability.
    Insufficient { available: i64 },
}

/// Input for a reservation attempt.
pub struct ReserveInput {
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub cart_id: Uuid,
    pub line_id: Uuid,
    pub item_id: Uuid,
    pub qty: i64,
    pub expires_at: Option<DateTime<Utc>>,
}

type StateTuple = (String, String, i64, i64, i64, i64, String, String);

fn map_state_row(row: StateTuple) -> InventoryStateRow {
    let (
        store_id,
        item_id,
        hq_baseline_qty,
        reserved_qty,
        sold_since_sync_qty,
        local_adjust_qty,
        baseline_as_of,
        updated_at,
    ) = row;
    InventoryStateRow {
        store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
        item_id: Uuid::parse_str(&item_id).unwrap_or_default(),
        hq_baseline_qty,
        reserved_qty,
        sold_since_sync_qty,
        local_adjust_qty,
        baseline_as_of: DateTime::parse_from_rfc3339(&baseline_as_of)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    }
}

const SELECT_STATE_COLS: &str = "store_id, item_id, hq_baseline_qty, reserved_qty, \
     sold_since_sync_qty, local_adjust_qty, baseline_as_of, updated_at";

/// Fetch the ledger state for one item, or `None` if untracked.
pub async fn get_inventory_state(
    pool: &SqlitePool,
    store_id: Uuid,
    item_id: Uuid,
) -> Result<Option<InventoryStateRow>, PoolError> {
    let row = sqlx::query_as::<_, StateTuple>(&format!(
        "SELECT {SELECT_STATE_COLS} FROM inventory_state WHERE store_id = ? AND item_id = ?"
    ))
    .bind(store_id.to_string())
    .bind(item_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(map_state_row))
}

/// Seed `inventory_state` rows from `catalog_items.available_qty` for tracked items
/// (non-NULL qty) that do not yet have a ledger row. Idempotent.
pub async fn seed_inventory_from_catalog(
    pool: &SqlitePool,
    store_id: Uuid,
) -> Result<u64, PoolError> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "INSERT INTO inventory_state \
         (store_id, item_id, hq_baseline_qty, reserved_qty, sold_since_sync_qty, local_adjust_qty, baseline_as_of, updated_at) \
         SELECT store_id, id, available_qty, 0, 0, 0, ?, ? \
         FROM catalog_items \
         WHERE store_id = ? AND available_qty IS NOT NULL \
           AND id NOT IN (SELECT item_id FROM inventory_state WHERE store_id = ?)",
    )
    .bind(&now)
    .bind(&now)
    .bind(store_id.to_string())
    .bind(store_id.to_string())
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Ensure a tracked item has an `inventory_state` row, seeding it from the given baseline
/// if absent. Idempotent and safe to call on the hot add-to-cart path so oversell
/// protection holds even before the startup/sync seeding has run. Returns true if a row
/// was created.
pub async fn ensure_inventory_state(
    pool: &SqlitePool,
    store_id: Uuid,
    item_id: Uuid,
    baseline_qty: i64,
) -> Result<bool, PoolError> {
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "INSERT OR IGNORE INTO inventory_state \
         (store_id, item_id, hq_baseline_qty, reserved_qty, sold_since_sync_qty, local_adjust_qty, baseline_as_of, updated_at) \
         VALUES (?, ?, ?, 0, 0, 0, ?, ?)",
    )
    .bind(store_id.to_string())
    .bind(item_id.to_string())
    .bind(baseline_qty)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Batch availability lookup for a set of items. Items without a ledger row (untracked)
/// are omitted from the returned map.
pub async fn available_to_sell_for_items(
    pool: &SqlitePool,
    store_id: Uuid,
    item_ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, i64>, PoolError> {
    let mut out = std::collections::HashMap::new();
    for item_id in item_ids {
        if let Some(state) = get_inventory_state(pool, store_id, *item_id).await? {
            out.insert(*item_id, state.available_to_sell());
        }
    }
    Ok(out)
}

/// List every tracked item's current `available_to_sell` for a store.
///
/// Used by the stream snapshot endpoint so a reconnecting client that missed events can
/// refetch full stock truth in one call.
pub async fn list_available_to_sell(
    pool: &SqlitePool,
    store_id: Uuid,
) -> Result<Vec<(Uuid, i64)>, PoolError> {
    let rows = sqlx::query_as::<_, (String, i64, i64, i64, i64)>(
        "SELECT item_id, hq_baseline_qty, reserved_qty, sold_since_sync_qty, local_adjust_qty \
         FROM inventory_state WHERE store_id = ? ORDER BY item_id",
    )
    .bind(store_id.to_string())
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (item_id, baseline, reserved, sold, adjust) in rows {
        if let Ok(id) = Uuid::parse_str(&item_id) {
            let available = (baseline + adjust - reserved - sold).max(0);
            out.push((id, available));
        }
    }
    Ok(out)
}

/// Attempt to reserve `qty` units for a cart line. Atomic: a single guarded UPDATE
/// prevents two concurrent reservations from both succeeding on the last unit.
pub async fn try_reserve(
    pool: &SqlitePool,
    input: ReserveInput,
) -> Result<ReserveOutcome, PoolError> {
    if input.qty <= 0 {
        return Ok(ReserveOutcome::Insufficient { available: 0 });
    }
    let mut tx = pool.begin().await?;

    // Untracked items have no ledger row → no constraint.
    let existing = sqlx::query_as::<_, (i64,)>(
        "SELECT 1 FROM inventory_state WHERE store_id = ? AND item_id = ?",
    )
    .bind(input.store_id.to_string())
    .bind(input.item_id.to_string())
    .fetch_optional(&mut *tx)
    .await?;
    if existing.is_none() {
        tx.commit().await?;
        return Ok(ReserveOutcome::Untracked);
    }

    let now = Utc::now().to_rfc3339();
    let updated = sqlx::query(
        "UPDATE inventory_state SET reserved_qty = reserved_qty + ?, updated_at = ? \
         WHERE store_id = ? AND item_id = ? \
           AND (hq_baseline_qty + local_adjust_qty - reserved_qty - sold_since_sync_qty) >= ?",
    )
    .bind(input.qty)
    .bind(&now)
    .bind(input.store_id.to_string())
    .bind(input.item_id.to_string())
    .bind(input.qty)
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        // Not enough available; report current availability.
        let avail = sqlx::query_as::<_, (i64,)>(
            "SELECT MAX(0, hq_baseline_qty + local_adjust_qty - reserved_qty - sold_since_sync_qty) \
             FROM inventory_state WHERE store_id = ? AND item_id = ?",
        )
        .bind(input.store_id.to_string())
        .bind(input.item_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(ReserveOutcome::Insufficient { available: avail.0 });
    }

    sqlx::query(
        "INSERT INTO stock_reservations \
         (id, store_id, register_id, cart_id, line_id, item_id, qty, state, expires_at, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(input.store_id.to_string())
    .bind(input.register_id.to_string())
    .bind(input.cart_id.to_string())
    .bind(input.line_id.to_string())
    .bind(input.item_id.to_string())
    .bind(input.qty)
    .bind(input.expires_at.map(|d| d.to_rfc3339()))
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(ReserveOutcome::Reserved)
}

/// Release the active reservation(s) for a single cart line (e.g. line removed or qty reduced).
/// Returns the number of units released back to availability.
pub async fn release_line_reservation(
    pool: &SqlitePool,
    store_id: Uuid,
    line_id: Uuid,
) -> Result<i64, PoolError> {
    let mut tx = pool.begin().await?;
    let now = Utc::now().to_rfc3339();
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT item_id, qty FROM stock_reservations \
         WHERE store_id = ? AND line_id = ? AND state = 'active'",
    )
    .bind(store_id.to_string())
    .bind(line_id.to_string())
    .fetch_all(&mut *tx)
    .await?;

    let mut released = 0i64;
    for (item_id, qty) in &rows {
        sqlx::query(
            "UPDATE inventory_state SET reserved_qty = MAX(0, reserved_qty - ?), updated_at = ? \
             WHERE store_id = ? AND item_id = ?",
        )
        .bind(*qty)
        .bind(&now)
        .bind(store_id.to_string())
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
        released += *qty;
    }
    sqlx::query(
        "UPDATE stock_reservations SET state = 'released', updated_at = ? \
         WHERE store_id = ? AND line_id = ? AND state = 'active'",
    )
    .bind(&now)
    .bind(store_id.to_string())
    .bind(line_id.to_string())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(released)
}

/// Release all active reservations for a cart (e.g. cart voided). Returns units released.
pub async fn release_cart_reservations(
    pool: &SqlitePool,
    store_id: Uuid,
    cart_id: Uuid,
) -> Result<i64, PoolError> {
    let mut tx = pool.begin().await?;
    let now = Utc::now().to_rfc3339();
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT item_id, qty FROM stock_reservations \
         WHERE store_id = ? AND cart_id = ? AND state = 'active'",
    )
    .bind(store_id.to_string())
    .bind(cart_id.to_string())
    .fetch_all(&mut *tx)
    .await?;
    let mut released = 0i64;
    for (item_id, qty) in &rows {
        sqlx::query(
            "UPDATE inventory_state SET reserved_qty = MAX(0, reserved_qty - ?), updated_at = ? \
             WHERE store_id = ? AND item_id = ?",
        )
        .bind(*qty)
        .bind(&now)
        .bind(store_id.to_string())
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
        released += *qty;
    }
    sqlx::query(
        "UPDATE stock_reservations SET state = 'released', updated_at = ? \
         WHERE store_id = ? AND cart_id = ? AND state = 'active'",
    )
    .bind(&now)
    .bind(store_id.to_string())
    .bind(cart_id.to_string())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(released)
}

/// Commit a cart's active reservations as sales: `reserved -= qty`, `sold_since_sync += qty`.
/// Returns units committed. Called on order finalization.
pub async fn commit_cart_sale(
    pool: &SqlitePool,
    store_id: Uuid,
    cart_id: Uuid,
) -> Result<i64, PoolError> {
    let mut tx = pool.begin().await?;
    let now = Utc::now().to_rfc3339();
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT item_id, qty FROM stock_reservations \
         WHERE store_id = ? AND cart_id = ? AND state = 'active'",
    )
    .bind(store_id.to_string())
    .bind(cart_id.to_string())
    .fetch_all(&mut *tx)
    .await?;
    let mut committed = 0i64;
    for (item_id, qty) in &rows {
        sqlx::query(
            "UPDATE inventory_state SET reserved_qty = MAX(0, reserved_qty - ?), \
             sold_since_sync_qty = sold_since_sync_qty + ?, updated_at = ? \
             WHERE store_id = ? AND item_id = ?",
        )
        .bind(*qty)
        .bind(*qty)
        .bind(&now)
        .bind(store_id.to_string())
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
        committed += *qty;
    }
    sqlx::query(
        "UPDATE stock_reservations SET state = 'committed', updated_at = ? \
         WHERE store_id = ? AND cart_id = ? AND state = 'active'",
    )
    .bind(&now)
    .bind(store_id.to_string())
    .bind(cart_id.to_string())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(committed)
}

/// Apply a net local stock delta (receive/adjust/transfer/returns restock) to a tracked item.
/// No-op for untracked items (no ledger row). Returns the new availability, or `None` if untracked.
pub async fn apply_local_delta(
    pool: &SqlitePool,
    store_id: Uuid,
    item_id: Uuid,
    delta: i64,
) -> Result<Option<i64>, PoolError> {
    let now = Utc::now().to_rfc3339();
    let updated = sqlx::query(
        "UPDATE inventory_state SET local_adjust_qty = local_adjust_qty + ?, updated_at = ? \
         WHERE store_id = ? AND item_id = ?",
    )
    .bind(delta)
    .bind(&now)
    .bind(store_id.to_string())
    .bind(item_id.to_string())
    .execute(pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(None);
    }
    let state = get_inventory_state(pool, store_id, item_id).await?;
    Ok(state.map(|s| s.available_to_sell()))
}

/// Rebase an item's HQ baseline from a fresh inventory sync.
///
/// Keeps active reservations (open carts). Recomputes `sold_since_sync_qty` and
/// `local_adjust_qty` from only the local activity *not yet reflected* in HQ's new
/// baseline (events newer than `reflected_through`), so a sale HQ hasn't seen yet is
/// never double-counted into availability. Older committed sales / movements are
/// considered reflected by HQ and dropped from the local counters.
pub async fn rebase_baseline(
    pool: &SqlitePool,
    store_id: Uuid,
    item_id: Uuid,
    new_baseline_qty: i64,
    reflected_through: DateTime<Utc>,
) -> Result<(), PoolError> {
    let mut tx = pool.begin().await?;
    let now = Utc::now().to_rfc3339();
    let cutoff = reflected_through.to_rfc3339();

    let unreflected_sold = sqlx::query_as::<_, (i64,)>(
        "SELECT COALESCE(SUM(qty), 0) FROM stock_reservations \
         WHERE store_id = ? AND item_id = ? AND state = 'committed' AND updated_at > ?",
    )
    .bind(store_id.to_string())
    .bind(item_id.to_string())
    .bind(&cutoff)
    .fetch_one(&mut *tx)
    .await?
    .0;

    let unreflected_adjust = sqlx::query_as::<_, (i64,)>(
        "SELECT COALESCE(SUM(quantity_delta), 0) FROM stock_movements \
         WHERE store_id = ? AND item_id = ? AND created_at > ?",
    )
    .bind(store_id.to_string())
    .bind(item_id.to_string())
    .bind(&cutoff)
    .fetch_one(&mut *tx)
    .await?
    .0;

    // Preserve active reservations on the existing row; default to 0 on first insert.
    sqlx::query(
        "INSERT INTO inventory_state \
         (store_id, item_id, hq_baseline_qty, reserved_qty, sold_since_sync_qty, local_adjust_qty, baseline_as_of, updated_at) \
         VALUES (?, ?, ?, 0, ?, ?, ?, ?) \
         ON CONFLICT(store_id, item_id) DO UPDATE SET \
            hq_baseline_qty = excluded.hq_baseline_qty, \
            sold_since_sync_qty = excluded.sold_since_sync_qty, \
            local_adjust_qty = excluded.local_adjust_qty, \
            baseline_as_of = excluded.baseline_as_of, \
            updated_at = excluded.updated_at",
    )
    .bind(store_id.to_string())
    .bind(item_id.to_string())
    .bind(new_baseline_qty)
    .bind(unreflected_sold)
    .bind(unreflected_adjust)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Reconcile a batch of HQ inventory levels: refresh the catalog snapshot and rebase the
/// ledger baseline for each item, keeping active reservations and unreflected local activity.
///
/// `reflected_through` for each item is its previous `baseline_as_of` (or the epoch on first
/// sight), so committed sales / stock movements newer than the last baseline are retained and
/// never double-counted. Reports `drift` for items where local activity exceeds the new HQ
/// baseline (availability clamped to zero) — a real discrepancy worth auditing.
pub async fn reconcile_inventory_levels(
    pool: &SqlitePool,
    store_id: Uuid,
    levels: &[InventoryLevel],
) -> Result<ReconcileSummary, PoolError> {
    // Refresh the synced snapshot (available_qty / is_available / image_urls).
    crate::catalog::replace_inventory_levels(pool, store_id, levels).await?;

    let epoch = DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_else(Utc::now);
    let mut summary = ReconcileSummary::default();
    for level in levels {
        let prev = get_inventory_state(pool, store_id, level.item_id).await?;
        let reflected_through = prev.map(|s| s.baseline_as_of).unwrap_or(epoch);
        rebase_baseline(
            pool,
            store_id,
            level.item_id,
            level.available_qty,
            reflected_through,
        )
        .await?;
        summary.items += 1;
        if let Some(after) = get_inventory_state(pool, store_id, level.item_id).await? {
            let raw = after.hq_baseline_qty + after.local_adjust_qty
                - after.reserved_qty
                - after.sold_since_sync_qty;
            if raw < 0 {
                summary.drift += 1;
            }
        }
    }
    Ok(summary)
}

/// Expire active reservations whose `expires_at` has passed, releasing their held stock.
/// Returns the number of reservations expired. Used by the TTL sweeper.
pub async fn expire_stale_reservations(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<u64, PoolError> {
    let mut tx = pool.begin().await?;
    let now_str = now.to_rfc3339();
    let rows = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT store_id, item_id, qty FROM stock_reservations \
         WHERE state = 'active' AND expires_at IS NOT NULL AND expires_at <= ?",
    )
    .bind(&now_str)
    .fetch_all(&mut *tx)
    .await?;
    for (store_id, item_id, qty) in &rows {
        sqlx::query(
            "UPDATE inventory_state SET reserved_qty = MAX(0, reserved_qty - ?), updated_at = ? \
             WHERE store_id = ? AND item_id = ?",
        )
        .bind(*qty)
        .bind(&now_str)
        .bind(store_id)
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
    }
    let res = sqlx::query(
        "UPDATE stock_reservations SET state = 'expired', updated_at = ? \
         WHERE state = 'active' AND expires_at IS NOT NULL AND expires_at <= ?",
    )
    .bind(&now_str)
    .bind(&now_str)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::run_migrations;
    use crate::pool::create_sqlite_pool;

    async fn pool_with_item(available_qty: Option<i64>) -> (SqlitePool, Uuid, Uuid) {
        let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let store_id = Uuid::nil();
        let item_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO catalog_items (id, store_id, sku, name, category_id, tax_category_id, is_active, available_qty) \
             VALUES (?, ?, 'SKU-1', 'Widget', ?, ?, 1, ?)",
        )
        .bind(item_id.to_string())
        .bind(store_id.to_string())
        .bind(cat.to_string())
        .bind(cat.to_string())
        .bind(available_qty)
        .execute(&pool)
        .await
        .unwrap();
        (pool, store_id, item_id)
    }

    fn reserve_input(store_id: Uuid, item_id: Uuid, qty: i64) -> ReserveInput {
        ReserveInput {
            store_id,
            register_id: Uuid::nil(),
            cart_id: Uuid::new_v4(),
            line_id: Uuid::new_v4(),
            item_id,
            qty,
            expires_at: None,
        }
    }

    #[tokio::test]
    async fn seed_creates_rows_only_for_tracked_items() {
        let (pool, store_id, item_id) = pool_with_item(Some(7)).await;
        let n = seed_inventory_from_catalog(&pool, store_id).await.unwrap();
        assert_eq!(n, 1);
        let state = get_inventory_state(&pool, store_id, item_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.hq_baseline_qty, 7);
        assert_eq!(state.available_to_sell(), 7);
        // Idempotent.
        let n2 = seed_inventory_from_catalog(&pool, store_id).await.unwrap();
        assert_eq!(n2, 0);
    }

    #[tokio::test]
    async fn untracked_item_reserves_without_constraint() {
        let (pool, store_id, item_id) = pool_with_item(None).await;
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();
        let outcome = try_reserve(&pool, reserve_input(store_id, item_id, 1000))
            .await
            .unwrap();
        assert_eq!(outcome, ReserveOutcome::Untracked);
    }

    #[tokio::test]
    async fn reserve_reduces_availability_and_blocks_oversell() {
        let (pool, store_id, item_id) = pool_with_item(Some(3)).await;
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();

        assert_eq!(
            try_reserve(&pool, reserve_input(store_id, item_id, 2))
                .await
                .unwrap(),
            ReserveOutcome::Reserved
        );
        let state = get_inventory_state(&pool, store_id, item_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.available_to_sell(), 1);

        // Asking for 2 more must fail (only 1 left).
        assert_eq!(
            try_reserve(&pool, reserve_input(store_id, item_id, 2))
                .await
                .unwrap(),
            ReserveOutcome::Insufficient { available: 1 }
        );
    }

    #[tokio::test]
    async fn release_returns_units_to_availability() {
        let (pool, store_id, item_id) = pool_with_item(Some(5)).await;
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();
        let line_id = Uuid::new_v4();
        let mut input = reserve_input(store_id, item_id, 4);
        input.line_id = line_id;
        try_reserve(&pool, input).await.unwrap();
        assert_eq!(
            get_inventory_state(&pool, store_id, item_id)
                .await
                .unwrap()
                .unwrap()
                .available_to_sell(),
            1
        );
        let released = release_line_reservation(&pool, store_id, line_id)
            .await
            .unwrap();
        assert_eq!(released, 4);
        assert_eq!(
            get_inventory_state(&pool, store_id, item_id)
                .await
                .unwrap()
                .unwrap()
                .available_to_sell(),
            5
        );
    }

    #[tokio::test]
    async fn commit_moves_reserved_to_sold() {
        let (pool, store_id, item_id) = pool_with_item(Some(5)).await;
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();
        let cart_id = Uuid::new_v4();
        let mut input = reserve_input(store_id, item_id, 2);
        input.cart_id = cart_id;
        try_reserve(&pool, input).await.unwrap();
        let committed = commit_cart_sale(&pool, store_id, cart_id).await.unwrap();
        assert_eq!(committed, 2);
        let state = get_inventory_state(&pool, store_id, item_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.reserved_qty, 0);
        assert_eq!(state.sold_since_sync_qty, 2);
        assert_eq!(state.available_to_sell(), 3);
    }

    #[tokio::test]
    async fn local_delta_makes_received_stock_sellable() {
        let (pool, store_id, item_id) = pool_with_item(Some(1)).await;
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();
        let avail = apply_local_delta(&pool, store_id, item_id, 10)
            .await
            .unwrap();
        assert_eq!(avail, Some(11));
    }

    #[tokio::test]
    async fn rebase_keeps_reservations_and_unreflected_sales() {
        let (pool, store_id, item_id) = pool_with_item(Some(10)).await;
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();

        // An open reservation (active) must survive a rebase.
        let cart_open = Uuid::new_v4();
        let mut open = reserve_input(store_id, item_id, 1);
        open.cart_id = cart_open;
        try_reserve(&pool, open).await.unwrap();

        // A committed sale AFTER the cutoff must be retained as unreflected.
        let cart_sold = Uuid::new_v4();
        let mut sold = reserve_input(store_id, item_id, 2);
        sold.cart_id = cart_sold;
        try_reserve(&pool, sold).await.unwrap();
        commit_cart_sale(&pool, store_id, cart_sold).await.unwrap();

        // Rebase with a cutoff in the past → the committed sale is "unreflected".
        let cutoff = Utc::now() - chrono::Duration::hours(1);
        rebase_baseline(&pool, store_id, item_id, 8, cutoff)
            .await
            .unwrap();

        let state = get_inventory_state(&pool, store_id, item_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.hq_baseline_qty, 8);
        assert_eq!(state.reserved_qty, 1, "active reservation preserved");
        assert_eq!(state.sold_since_sync_qty, 2, "unreflected sale retained");
        // available = 8 + 0 - 1 - 2 = 5
        assert_eq!(state.available_to_sell(), 5);
    }

    #[tokio::test]
    async fn concurrent_reservations_never_oversell() {
        use sqlx::sqlite::SqlitePoolOptions;
        // Single shared connection so SQLite serializes the guarded UPDATEs deterministically.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        let store_id = Uuid::nil();
        let item_id = Uuid::new_v4();
        let cat = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO catalog_items (id, store_id, sku, name, category_id, tax_category_id, is_active, available_qty) \
             VALUES (?, ?, 'SKU-C', 'Widget', ?, ?, 1, 5)",
        )
        .bind(item_id.to_string())
        .bind(store_id.to_string())
        .bind(cat.to_string())
        .bind(cat.to_string())
        .execute(&pool)
        .await
        .unwrap();
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();

        let capacity = 5;
        let attempts = 25;
        let mut handles = Vec::new();
        for _ in 0..attempts {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                try_reserve(&pool, reserve_input(store_id, item_id, 1))
                    .await
                    .unwrap()
            }));
        }
        let mut reserved = 0;
        for h in handles {
            if matches!(h.await.unwrap(), ReserveOutcome::Reserved) {
                reserved += 1;
            }
        }
        assert_eq!(
            reserved, capacity,
            "exactly capacity reservations may succeed"
        );
        let state = get_inventory_state(&pool, store_id, item_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.reserved_qty, capacity);
        assert_eq!(state.available_to_sell(), 0);
    }

    #[tokio::test]
    async fn reconcile_rebases_baseline_and_preserves_reservations() {
        let (pool, store_id, item_id) = pool_with_item(Some(10)).await;
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();
        // An open reservation must survive HQ reconcile.
        let cart = Uuid::new_v4();
        let mut r = reserve_input(store_id, item_id, 2);
        r.cart_id = cart;
        try_reserve(&pool, r).await.unwrap();

        let levels = vec![InventoryLevel {
            item_id,
            available_qty: 6,
            is_available: true,
            image_urls: vec![],
            version: 2,
        }];
        let summary = reconcile_inventory_levels(&pool, store_id, &levels)
            .await
            .unwrap();
        assert_eq!(summary.items, 1);
        assert_eq!(summary.drift, 0);

        let s = get_inventory_state(&pool, store_id, item_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s.hq_baseline_qty, 6);
        assert_eq!(
            s.reserved_qty, 2,
            "active reservation preserved across rebase"
        );
        assert_eq!(s.available_to_sell(), 4);
    }

    #[tokio::test]
    async fn reconcile_flags_drift_when_local_sales_exceed_baseline() {
        let (pool, store_id, item_id) = pool_with_item(Some(10)).await;
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();
        // Ensure the commit timestamp is strictly after the baseline cutoff.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let cart = Uuid::new_v4();
        let mut r = reserve_input(store_id, item_id, 5);
        r.cart_id = cart;
        try_reserve(&pool, r).await.unwrap();
        commit_cart_sale(&pool, store_id, cart).await.unwrap();

        // HQ reports only 2 on hand, but we have 5 unreflected local sales → drift.
        let levels = vec![InventoryLevel {
            item_id,
            available_qty: 2,
            is_available: true,
            image_urls: vec![],
            version: 2,
        }];
        let summary = reconcile_inventory_levels(&pool, store_id, &levels)
            .await
            .unwrap();
        assert_eq!(
            summary.drift, 1,
            "local sales exceeding HQ baseline flag drift"
        );
        let s = get_inventory_state(&pool, store_id, item_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s.sold_since_sync_qty, 5, "unreflected sale retained");
        assert_eq!(s.available_to_sell(), 0, "availability clamped at zero");
    }

    #[tokio::test]
    async fn expire_releases_stale_reservations() {
        let (pool, store_id, item_id) = pool_with_item(Some(5)).await;
        seed_inventory_from_catalog(&pool, store_id).await.unwrap();
        let mut input = reserve_input(store_id, item_id, 3);
        input.expires_at = Some(Utc::now() - chrono::Duration::minutes(1));
        try_reserve(&pool, input).await.unwrap();
        assert_eq!(
            get_inventory_state(&pool, store_id, item_id)
                .await
                .unwrap()
                .unwrap()
                .available_to_sell(),
            2
        );
        let expired = expire_stale_reservations(&pool, Utc::now()).await.unwrap();
        assert_eq!(expired, 1);
        assert_eq!(
            get_inventory_state(&pool, store_id, item_id)
                .await
                .unwrap()
                .unwrap()
                .available_to_sell(),
            5
        );
    }
}
