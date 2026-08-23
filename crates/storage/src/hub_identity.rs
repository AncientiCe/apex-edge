//! Durable store/register identity for this hub process.

use apex_edge_metrics::{
    DB_OPERATIONS_TOTAL, DB_OPERATION_DURATION_SECONDS, DB_OUTCOME_ERROR, DB_OUTCOME_SUCCESS,
};
use chrono::Utc;
use sqlx::SqlitePool;
use std::time::Instant;
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubIdentity {
    pub store_id: Uuid,
    pub register_id: Uuid,
}

fn db_outcome<T>(result: &Result<T, sqlx::Error>) -> &'static str {
    if result.is_ok() {
        DB_OUTCOME_SUCCESS
    } else {
        DB_OUTCOME_ERROR
    }
}

async fn load_identity(pool: &SqlitePool) -> Result<Option<HubIdentity>, PoolError> {
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT store_id, register_id FROM hub_identity WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(store, register)| {
        Some(HubIdentity {
            store_id: Uuid::parse_str(&store).ok()?,
            register_id: Uuid::parse_str(&register).ok()?,
        })
    }))
}

async fn persist_identity(pool: &SqlitePool, identity: HubIdentity) -> Result<(), PoolError> {
    const OP: &str = "persist_hub_identity";
    let start = Instant::now();
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        "INSERT INTO hub_identity (id, store_id, register_id, created_at, updated_at)
         VALUES (1, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET store_id = excluded.store_id,
              register_id = excluded.register_id, updated_at = excluded.updated_at",
    )
    .bind(identity.store_id.to_string())
    .bind(identity.register_id.to_string())
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, "operation" => OP, "outcome" => db_outcome(&result))
        .increment(1);
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, "operation" => OP)
        .record(start.elapsed().as_secs_f64());
    result?;
    Ok(())
}

/// Resolve the hub's store and register.
///
/// Env IDs win and are persisted. If neither env nor the database has an identity,
/// one is generated so a restart without env still serves the same store.
pub async fn resolve_hub_identity(
    pool: &SqlitePool,
    env_store: Option<Uuid>,
    env_register: Option<Uuid>,
) -> Result<HubIdentity, PoolError> {
    const OP: &str = "resolve_hub_identity";
    let start = Instant::now();
    let stored = load_identity(pool).await?;
    let identity = match (env_store, env_register, stored) {
        (Some(store_id), Some(register_id), _) => HubIdentity {
            store_id,
            register_id,
        },
        (Some(store_id), None, Some(existing)) => HubIdentity {
            store_id,
            register_id: existing.register_id,
        },
        (None, Some(register_id), Some(existing)) => HubIdentity {
            store_id: existing.store_id,
            register_id,
        },
        (Some(store_id), None, None) => HubIdentity {
            store_id,
            register_id: Uuid::new_v4(),
        },
        (None, Some(register_id), None) => HubIdentity {
            store_id: Uuid::new_v4(),
            register_id,
        },
        (None, None, Some(existing)) => existing,
        (None, None, None) => HubIdentity {
            store_id: Uuid::new_v4(),
            register_id: Uuid::new_v4(),
        },
    };
    persist_identity(pool, identity).await?;
    metrics::counter!(DB_OPERATIONS_TOTAL, "operation" => OP, "outcome" => DB_OUTCOME_SUCCESS)
        .increment(1);
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, "operation" => OP)
        .record(start.elapsed().as_secs_f64());
    Ok(identity)
}

pub fn parse_uuid_env(name: &str) -> Option<Uuid> {
    std::env::var(name)
        .ok()
        .and_then(|s| Uuid::parse_str(s.trim()).ok())
}
