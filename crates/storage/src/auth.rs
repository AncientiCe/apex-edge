//! Auth persistence: pairing codes, trusted devices, sessions, associate cache.

use apex_edge_metrics::{
    DB_OPERATIONS_TOTAL, DB_OPERATION_DURATION_SECONDS, DB_OUTCOME_ERROR, DB_OUTCOME_SUCCESS,
};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::time::Instant;
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone)]
pub struct DevicePairingCodeRow {
    pub id: Uuid,
    pub store_id: Uuid,
    pub code_hash: String,
    pub created_by: String,
    pub expires_at: DateTime<Utc>,
    pub attempts: i64,
    pub max_attempts: i64,
    pub consumed_at: Option<DateTime<Utc>>,
    pub consumed_device_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct TrustedDeviceRow {
    pub id: Uuid,
    pub store_id: Uuid,
    pub device_name: String,
    pub platform: Option<String>,
    pub secret_hash: String,
    pub status: String,
    pub enrolled_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct AuthSessionRow {
    pub session_id: Uuid,
    pub associate_id: String,
    pub store_id: Uuid,
    pub device_id: Uuid,
    pub access_exp: DateTime<Utc>,
    pub refresh_exp: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn db_outcome<T>(result: &Result<T, sqlx::Error>) -> &'static str {
    if result.is_ok() {
        DB_OUTCOME_SUCCESS
    } else {
        DB_OUTCOME_ERROR
    }
}

pub async fn create_device_pairing_code(
    pool: &SqlitePool,
    store_id: Uuid,
    code_hash: &str,
    created_by: &str,
    expires_at: DateTime<Utc>,
    max_attempts: i64,
) -> Result<Uuid, PoolError> {
    const OP: &str = "create_device_pairing_code";
    let start = Instant::now();
    let id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        "INSERT INTO device_pairing_codes
        (id, store_id, code_hash, created_by, expires_at, attempts, max_attempts, created_at)
        VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(code_hash)
    .bind(created_by)
    .bind(expires_at.to_rfc3339())
    .bind(max_attempts)
    .bind(now)
    .execute(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    result?;
    Ok(id)
}

pub async fn get_pairing_code_by_hash(
    pool: &SqlitePool,
    code_hash: &str,
) -> Result<Option<DevicePairingCodeRow>, PoolError> {
    const OP: &str = "get_pairing_code_by_hash";
    let start = Instant::now();
    let result = sqlx::query_as::<_, (String, String, String, String, String, i64, i64, Option<String>, Option<String>)>(
        "SELECT id, store_id, code_hash, created_by, expires_at, attempts, max_attempts, consumed_at, consumed_device_id
         FROM device_pairing_codes
         WHERE code_hash = ?"
    )
    .bind(code_hash)
    .fetch_optional(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    let row = result?.map(
        |(
            id,
            store_id,
            code_hash,
            created_by,
            expires_at,
            attempts,
            max_attempts,
            consumed_at,
            consumed_device_id,
        )| DevicePairingCodeRow {
            id: Uuid::parse_str(&id).unwrap_or_default(),
            store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
            code_hash,
            created_by,
            expires_at: parse_dt(&expires_at),
            attempts,
            max_attempts,
            consumed_at: consumed_at.as_deref().map(parse_dt),
            consumed_device_id: consumed_device_id.and_then(|v| Uuid::parse_str(&v).ok()),
        },
    );
    Ok(row)
}

pub async fn increment_pairing_code_attempts(pool: &SqlitePool, id: Uuid) -> Result<(), PoolError> {
    const OP: &str = "increment_pairing_code_attempts";
    let start = Instant::now();
    let result = sqlx::query(
        "UPDATE device_pairing_codes
         SET attempts = attempts + 1
         WHERE id = ?",
    )
    .bind(id.to_string())
    .execute(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    result?;
    Ok(())
}

pub async fn consume_pairing_code(
    pool: &SqlitePool,
    id: Uuid,
    device_id: Uuid,
) -> Result<(), PoolError> {
    const OP: &str = "consume_pairing_code";
    let start = Instant::now();
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        "UPDATE device_pairing_codes
         SET consumed_at = ?, consumed_device_id = ?
         WHERE id = ?",
    )
    .bind(now)
    .bind(device_id.to_string())
    .bind(id.to_string())
    .execute(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    result?;
    Ok(())
}

pub async fn create_trusted_device(
    pool: &SqlitePool,
    id: Uuid,
    store_id: Uuid,
    device_name: &str,
    platform: Option<&str>,
    secret_hash: &str,
) -> Result<(), PoolError> {
    const OP: &str = "create_trusted_device";
    let start = Instant::now();
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        "INSERT INTO trusted_devices (id, store_id, device_name, platform, secret_hash, status, enrolled_at)
         VALUES (?, ?, ?, ?, ?, 'active', ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(device_name)
    .bind(platform)
    .bind(secret_hash)
    .bind(now)
    .execute(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    result?;
    Ok(())
}

pub async fn get_trusted_device(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<Option<TrustedDeviceRow>, PoolError> {
    const OP: &str = "get_trusted_device";
    let start = Instant::now();
    let result = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            Option<String>,
        ),
    >(
        "SELECT id, store_id, device_name, platform, secret_hash, status, enrolled_at, revoked_at
         FROM trusted_devices
         WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    let row = result?.map(
        |(id, store_id, device_name, platform, secret_hash, status, enrolled_at, revoked_at)| {
            TrustedDeviceRow {
                id: Uuid::parse_str(&id).unwrap_or_default(),
                store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
                device_name,
                platform,
                secret_hash,
                status,
                enrolled_at: parse_dt(&enrolled_at),
                revoked_at: revoked_at.as_deref().map(parse_dt),
            }
        },
    );
    Ok(row)
}

pub async fn upsert_associate_identity(
    pool: &SqlitePool,
    associate_id: &str,
    store_id: Uuid,
    name: Option<&str>,
    email: Option<&str>,
    claims_json: &str,
) -> Result<(), PoolError> {
    const OP: &str = "upsert_associate_identity";
    let start = Instant::now();
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        "INSERT INTO associate_identities (associate_id, store_id, name, email, claims_json, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(associate_id, store_id)
         DO UPDATE SET name = excluded.name, email = excluded.email, claims_json = excluded.claims_json, updated_at = excluded.updated_at",
    )
    .bind(associate_id)
    .bind(store_id.to_string())
    .bind(name)
    .bind(email)
    .bind(claims_json)
    .bind(now)
    .execute(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    result?;
    Ok(())
}

pub async fn create_auth_session(
    pool: &SqlitePool,
    session_id: Uuid,
    associate_id: &str,
    store_id: Uuid,
    device_id: Uuid,
    access_exp: DateTime<Utc>,
    refresh_exp: DateTime<Utc>,
) -> Result<(), PoolError> {
    const OP: &str = "create_auth_session";
    let start = Instant::now();
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        "INSERT INTO auth_sessions (session_id, associate_id, store_id, device_id, access_exp, refresh_exp, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(session_id.to_string())
    .bind(associate_id)
    .bind(store_id.to_string())
    .bind(device_id.to_string())
    .bind(access_exp.to_rfc3339())
    .bind(refresh_exp.to_rfc3339())
    .bind(now)
    .execute(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    result?;
    Ok(())
}

pub async fn get_auth_session(
    pool: &SqlitePool,
    session_id: Uuid,
) -> Result<Option<AuthSessionRow>, PoolError> {
    const OP: &str = "get_auth_session";
    let start = Instant::now();
    let result = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
        ),
    >(
        "SELECT session_id, associate_id, store_id, device_id, access_exp, refresh_exp, revoked_at
         FROM auth_sessions
         WHERE session_id = ?",
    )
    .bind(session_id.to_string())
    .fetch_optional(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    let row = result?.map(
        |(session_id, associate_id, store_id, device_id, access_exp, refresh_exp, revoked_at)| {
            AuthSessionRow {
                session_id: Uuid::parse_str(&session_id).unwrap_or_default(),
                associate_id,
                store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
                device_id: Uuid::parse_str(&device_id).unwrap_or_default(),
                access_exp: parse_dt(&access_exp),
                refresh_exp: parse_dt(&refresh_exp),
                revoked_at: revoked_at.as_deref().map(parse_dt),
            }
        },
    );
    Ok(row)
}

pub async fn revoke_auth_session(pool: &SqlitePool, session_id: Uuid) -> Result<(), PoolError> {
    const OP: &str = "revoke_auth_session";
    let start = Instant::now();
    let now = Utc::now().to_rfc3339();
    let result = sqlx::query(
        "UPDATE auth_sessions
         SET revoked_at = ?
         WHERE session_id = ?",
    )
    .bind(now)
    .bind(session_id.to_string())
    .execute(pool)
    .await;
    metrics::counter!(DB_OPERATIONS_TOTAL, 1u64, "operation" => OP, "outcome" => db_outcome(&result));
    metrics::histogram!(DB_OPERATION_DURATION_SECONDS, start.elapsed().as_secs_f64(), "operation" => OP);
    result?;
    Ok(())
}
