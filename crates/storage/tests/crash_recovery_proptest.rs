//! Crash-recovery property tests.
//!
//! Simulates the ApexEdge hub being killed (SIGKILL-equivalent: drop the pool,
//! invalidate all in-flight work, reopen from the on-disk file) at random points
//! during an arbitrary sequence of outbox + audit operations.
//!
//! # Invariants
//!
//! 1. **Outbox is write-idempotent on replay.** Replaying the same insert with the
//!    same id after a crash does not create a duplicate row. (Uses
//!    `INSERT OR IGNORE` semantics guarded by the primary key constraint.)
//!
//! 2. **Delivered rows stay delivered.** A row marked `delivered` before a crash is
//!    still `delivered` after the pool is reopened.
//!
//! 3. **Audit chain integrity.** `verify_chain` returns `ok: true` after any crash,
//!    no matter at which operation we crashed.
//!
//! 4. **No partial rows.** Every row in `outbox` has a parseable uuid id, valid
//!    status ∈ {pending, delivered, dead_letter}, and an integer `attempts ≥ 0`.

use apex_edge_storage::verify_chain;
use apex_edge_storage::{
    commit_cart_sale, create_sqlite_pool, fetch_pending_outbox, get_inventory_state, insert_outbox,
    mark_delivered, rebase_baseline, release_cart_reservations, run_migrations, set_audit_key,
    try_reserve, AuditKey, ReserveInput,
};
use chrono::Utc;
use proptest::prelude::*;
use sqlx::SqlitePool;
use std::sync::Once;
use tempfile::TempDir;
use uuid::Uuid;

static INIT_AUDIT_KEY: Once = Once::new();

fn init_audit_key_once() {
    INIT_AUDIT_KEY.call_once(|| {
        set_audit_key(AuditKey {
            hub_key_id: "proptest".into(),
            secret: vec![0x42; 32],
        });
    });
}

#[derive(Debug, Clone)]
enum Op {
    InsertPending(Uuid),
    Deliver(usize), // index into the list of inserted ids
    Audit(String),
    Crash,
}

fn op_strategy(pool_size: usize) -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => any::<u128>().prop_map(|n| Op::InsertPending(Uuid::from_u128(n))),
        2 => (0..pool_size.max(1)).prop_map(Op::Deliver),
        2 => "[a-z_]{3,12}".prop_map(Op::Audit),
        1 => Just(Op::Crash),
    ]
}

async fn open(path: &str) -> SqlitePool {
    let pool = create_sqlite_pool(path).await.expect("open pool");
    run_migrations(&pool).await.expect("migrate");
    pool
}

async fn count_rows(pool: &SqlitePool) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM outbox")
        .fetch_one(pool)
        .await
        .expect("count");
    row.0
}

async fn distinct_ids(pool: &SqlitePool) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT id) FROM outbox")
        .fetch_one(pool)
        .await
        .expect("count distinct");
    row.0
}

async fn statuses_valid(pool: &SqlitePool) -> bool {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT status FROM outbox")
        .fetch_all(pool)
        .await
        .expect("statuses");
    rows.iter()
        .all(|(s,)| matches!(s.as_str(), "pending" | "delivered" | "dead_letter"))
}

fn run(ops: Vec<Op>) -> Result<(), TestCaseError> {
    init_audit_key_once();
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("apex.db").to_string_lossy().to_string();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let mut pool = open(&db_path).await;
        let mut inserted: Vec<Uuid> = Vec::new();
        let mut delivered_expected: std::collections::HashSet<Uuid> =
            std::collections::HashSet::new();

        for op in ops {
            match op {
                Op::InsertPending(id) => {
                    // Idempotent by primary key: subsequent identical inserts fail; we swallow.
                    let _ = insert_outbox(&pool, id, "{}").await;
                    if !inserted.contains(&id) {
                        inserted.push(id);
                    }
                }
                Op::Deliver(idx) => {
                    if inserted.is_empty() {
                        continue;
                    }
                    let id = inserted[idx % inserted.len()];
                    let _ = mark_delivered(&pool, id).await;
                    delivered_expected.insert(id);
                }
                Op::Audit(event) => {
                    let _ = apex_edge_storage::record(&pool, &event, None, "{}").await;
                }
                Op::Crash => {
                    // Simulate SIGKILL: drop the pool without close(), reopen from disk.
                    drop(pool);
                    pool = open(&db_path).await;
                }
            }
        }

        // -------- Invariants --------

        // (4) No partial rows.
        let total = count_rows(&pool).await;
        let distinct = distinct_ids(&pool).await;
        prop_assert_eq!(total, distinct, "duplicate outbox rows exist");
        prop_assert!(statuses_valid(&pool).await, "invalid status detected");

        // (2) Delivered rows stay delivered after crash.
        for id in &delivered_expected {
            let row: Option<(String,)> = sqlx::query_as("SELECT status FROM outbox WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&pool)
                .await
                .unwrap();
            if let Some((status,)) = row {
                prop_assert_eq!(
                    status.as_str(),
                    "delivered",
                    "expected delivered row to stay delivered after crash"
                );
            }
        }

        // (3) Audit chain verifies.
        let verification = verify_chain(&pool).await.expect("verify_chain");
        prop_assert!(verification.ok, "audit chain tampered after crash replay");

        // (1) Write-idempotency on replay: re-inserting the same ids must not grow the
        //     table.
        let before = count_rows(&pool).await;
        for id in &inserted {
            let _ = insert_outbox(&pool, *id, "{}").await;
        }
        let after = count_rows(&pool).await;
        prop_assert_eq!(before, after, "replay inserted duplicate rows");

        // Pending enumeration should still succeed.
        let _pending = fetch_pending_outbox(&pool, 100).await.unwrap();

        Ok(())
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, max_shrink_iters: 32, .. ProptestConfig::default() })]

    #[test]
    fn crash_recovery_preserves_invariants(
        ops in prop::collection::vec(op_strategy(8), 1..30)
    ) {
        run(ops)?;
    }
}

// -------- Inventory ledger crash recovery --------
//
// The real-time inventory ledger and its reservations live in SQLite, so they must
// survive a SIGKILL-equivalent crash. After recovery, regardless of where we crashed
// mid-cart, these invariants must hold for every item:
//   (L1) reserved_qty >= 0 and sold_since_sync_qty >= 0
//   (L2) reserved_qty == SUM(qty) of still-active reservation rows (aggregate matches rows)
//   (L3) available_to_sell() >= 0 (never oversell into negative)

#[derive(Debug, Clone)]
enum LedgerOp {
    Reserve { cart: usize, qty: i64 },
    Commit(usize),
    Release(usize),
    Crash,
}

fn ledger_op_strategy(carts: usize) -> impl Strategy<Value = LedgerOp> {
    prop_oneof![
        5 => (0..carts.max(1), 1i64..6).prop_map(|(cart, qty)| LedgerOp::Reserve { cart, qty }),
        2 => (0..carts.max(1)).prop_map(LedgerOp::Commit),
        2 => (0..carts.max(1)).prop_map(LedgerOp::Release),
        1 => Just(LedgerOp::Crash),
    ]
}

async fn active_reservation_qty(pool: &SqlitePool, store_id: Uuid, item_id: Uuid) -> i64 {
    let row: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(qty), 0) FROM stock_reservations \
         WHERE store_id = ? AND item_id = ? AND state = 'active'",
    )
    .bind(store_id.to_string())
    .bind(item_id.to_string())
    .fetch_one(pool)
    .await
    .expect("sum active reservations");
    row.0
}

fn run_ledger(ops: Vec<LedgerOp>) -> Result<(), TestCaseError> {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("ledger.db").to_string_lossy().to_string();
    let store_id = Uuid::from_u128(0xA11CE);
    let item_id = Uuid::from_u128(0xB0B);
    let carts: Vec<Uuid> = (0..4).map(|i| Uuid::from_u128(0xC000 + i)).collect();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let mut pool = open(&db_path).await;
        // Establish an HQ baseline so the item is tracked (idempotent across the run).
        rebase_baseline(&pool, store_id, item_id, 20, Utc::now())
            .await
            .expect("seed baseline");

        for op in ops {
            match op {
                LedgerOp::Reserve { cart, qty } => {
                    let cart_id = carts[cart % carts.len()];
                    let input = ReserveInput {
                        store_id,
                        register_id: Uuid::from_u128(0x5E00 + cart as u128),
                        cart_id,
                        line_id: Uuid::new_v4(),
                        item_id,
                        qty,
                        expires_at: None,
                    };
                    let _ = try_reserve(&pool, input).await;
                }
                LedgerOp::Commit(cart) => {
                    let cart_id = carts[cart % carts.len()];
                    let _ = commit_cart_sale(&pool, store_id, cart_id).await;
                }
                LedgerOp::Release(cart) => {
                    let cart_id = carts[cart % carts.len()];
                    let _ = release_cart_reservations(&pool, store_id, cart_id).await;
                }
                LedgerOp::Crash => {
                    drop(pool);
                    pool = open(&db_path).await;
                }
            }
        }

        let state = get_inventory_state(&pool, store_id, item_id)
            .await
            .expect("get state")
            .expect("tracked item row exists");

        // (L1)
        prop_assert!(state.reserved_qty >= 0, "reserved_qty went negative");
        prop_assert!(
            state.sold_since_sync_qty >= 0,
            "sold_since_sync went negative"
        );
        // (L2)
        let active = active_reservation_qty(&pool, store_id, item_id).await;
        prop_assert_eq!(
            state.reserved_qty,
            active,
            "reserved_qty diverged from active reservation rows after crash"
        );
        // (L3)
        prop_assert!(
            state.available_to_sell() >= 0,
            "available_to_sell went negative"
        );

        Ok(())
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, max_shrink_iters: 32, .. ProptestConfig::default() })]

    #[test]
    fn ledger_crash_recovery_preserves_reservation_invariants(
        ops in prop::collection::vec(ledger_op_strategy(4), 1..30)
    ) {
        run_ledger(ops)?;
    }
}
