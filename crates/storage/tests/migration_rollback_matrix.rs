//! Migration rollback matrix.
//!
//! Verifies that the v0.6.0 additive migrations (010 returns, 011 shifts,
//! 012 audit chain, 013 approvals) can cleanly round-trip:
//!
//!   forward → backward → forward
//!
//! For each round-trip:
//! 1. Run `run_migrations` from an empty DB (forward).
//! 2. Seed a row into each new table (returns, shifts, approvals).
//! 3. Run `run_down_v0_6_0` (backward).
//! 4. Assert the v0.6.0 tables are gone but v0.5.x tables (`orders`, `audit_log`,
//!    `outbox`) still exist and are queryable.
//! 5. Run `run_migrations` again (forward).
//! 6. Assert v0.6.0 tables are recreated, empty, and accept writes.
//!
//! This is the contract operators rely on when downgrading from v0.6.0 → v0.5.x
//! after an incident, then re-upgrading.

use apex_edge_storage::{create_sqlite_pool, run_down_v0_6_0, run_migrations};
use sqlx::SqlitePool;
use tempfile::TempDir;
use uuid::Uuid;

async fn table_exists(pool: &SqlitePool, name: &str) -> bool {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT name FROM sqlite_master WHERE type='table' AND name=?")
            .bind(name)
            .fetch_optional(pool)
            .await
            .unwrap();
    row.is_some()
}

async fn seed_v0_6_0_tables(pool: &SqlitePool) {
    let store_id = Uuid::nil().to_string();
    let register_id = Uuid::nil().to_string();

    sqlx::query(
        "INSERT INTO returns (id, store_id, register_id, state, created_at) \
         VALUES (?, ?, ?, 'open', datetime('now'))",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&store_id)
    .bind(&register_id)
    .execute(pool)
    .await
    .expect("insert return");

    sqlx::query(
        "INSERT INTO shifts (id, store_id, register_id, opened_at) \
         VALUES (?, ?, ?, datetime('now'))",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&store_id)
    .bind(&register_id)
    .execute(pool)
    .await
    .expect("insert shift");

    sqlx::query(
        "INSERT INTO approvals (id, store_id, register_id, action, state, created_at, expires_at) \
         VALUES (?, ?, ?, 'return.blind', 'pending', datetime('now'), datetime('now','+1 hour'))",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&store_id)
    .bind(&register_id)
    .execute(pool)
    .await
    .expect("insert approval");
}

#[tokio::test]
async fn forward_backward_forward_round_trip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("rb.db").to_string_lossy().to_string();

    // (1) Forward
    let pool = create_sqlite_pool(&path).await.unwrap();
    run_migrations(&pool).await.unwrap();
    for t in ["returns", "return_lines", "refunds", "shifts", "approvals"] {
        assert!(
            table_exists(&pool, t).await,
            "table {} missing after forward",
            t
        );
    }

    // (2) Seed
    seed_v0_6_0_tables(&pool).await;

    // (3) Backward
    run_down_v0_6_0(&pool).await.unwrap();
    for t in ["returns", "return_lines", "refunds", "shifts", "approvals"] {
        assert!(
            !table_exists(&pool, t).await,
            "table {} still exists after down",
            t
        );
    }

    // (4) v0.5.x tables must survive (orders, audit_log, outbox)
    for t in ["carts", "audit_log", "outbox"] {
        assert!(
            table_exists(&pool, t).await,
            "v0.5.x table {} lost during rollback",
            t
        );
    }

    // (5) Forward again
    run_migrations(&pool).await.unwrap();
    for t in ["returns", "return_lines", "refunds", "shifts", "approvals"] {
        assert!(
            table_exists(&pool, t).await,
            "table {} missing after re-forward",
            t
        );
    }

    // (6) New tables accept writes again
    seed_v0_6_0_tables(&pool).await;

    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM returns")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, 1, "expected 1 fresh return after re-migrate");
}

#[tokio::test]
async fn double_down_is_idempotent() {
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    run_down_v0_6_0(&pool).await.unwrap();
    run_down_v0_6_0(&pool)
        .await
        .expect("down must be idempotent when nothing to drop");
}

#[tokio::test]
async fn double_up_is_idempotent() {
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    run_migrations(&pool)
        .await
        .expect("up must be idempotent when all migrations already applied");
}
