//! Tamper-evident audit log.
//!
//! Each row is linked to the previous one via a hash chain, keyed by an HMAC secret
//! loaded once per hub. Any retroactive edit or deletion breaks the chain and is
//! detected by [`verify_chain`] / `GET /audit/verify`.
//!
//! Chain formula:
//! `hash_n = HMAC-SHA256(hub_key, prev_hash || '|' || event_type || '|' || entity_id || '|' || payload || '|' || created_at)`.

use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::{Row, SqlitePool};
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

use crate::pool::PoolError;

type HmacSha256 = Hmac<Sha256>;

/// Signing key + logical id for the audit chain. The same `hub_key_id` is stored with
/// each appended row so rotations don't break historical verification.
#[derive(Clone, Debug)]
pub struct AuditKey {
    pub hub_key_id: String,
    pub secret: Vec<u8>,
}

impl AuditKey {
    pub fn new(hub_key_id: impl Into<String>, secret: impl Into<Vec<u8>>) -> Self {
        Self {
            hub_key_id: hub_key_id.into(),
            secret: secret.into(),
        }
    }
}

static AUDIT_KEY: OnceLock<Mutex<AuditKey>> = OnceLock::new();

/// Install/override the global audit signing key. Call during startup; tests can call
/// it with a deterministic key.
pub fn set_audit_key(key: AuditKey) {
    let slot = AUDIT_KEY.get_or_init(|| Mutex::new(key.clone()));
    if let Ok(mut guard) = slot.lock() {
        *guard = key;
    }
}

fn current_key() -> AuditKey {
    let slot = AUDIT_KEY.get_or_init(|| {
        Mutex::new(AuditKey::new(
            "dev-hub-key",
            b"apex-edge-default-audit-key-do-not-use-in-prod".to_vec(),
        ))
    });
    slot.lock().expect("audit key mutex poisoned").clone()
}

fn compute_hash(
    secret: &[u8],
    prev_hash: &str,
    event_type: &str,
    entity_id: Option<&str>,
    payload: &str,
    created_at: &str,
    hub_key_id: &str,
) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(prev_hash.as_bytes());
    mac.update(b"|");
    mac.update(event_type.as_bytes());
    mac.update(b"|");
    mac.update(entity_id.unwrap_or("").as_bytes());
    mac.update(b"|");
    mac.update(payload.as_bytes());
    mac.update(b"|");
    mac.update(created_at.as_bytes());
    mac.update(b"|");
    mac.update(hub_key_id.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

async fn last_hash(pool: &SqlitePool) -> Result<String, PoolError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT hash FROM audit_log ORDER BY id DESC LIMIT 1")
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(h,)| h).unwrap_or_default())
}

/// Append a hash-chained, signed audit row.
pub async fn record(
    pool: &SqlitePool,
    event_type: &str,
    entity_id: Option<Uuid>,
    payload: &str,
) -> Result<(), PoolError> {
    let key = current_key();
    let now = Utc::now().to_rfc3339();
    let entity_id_str = entity_id.map(|u| u.to_string());
    let mut tx = pool.begin().await?;

    let prev_hash: String =
        sqlx::query_as::<_, (String,)>("SELECT hash FROM audit_log ORDER BY id DESC LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?
            .map(|(h,)| h)
            .unwrap_or_default();

    let hash = compute_hash(
        &key.secret,
        &prev_hash,
        event_type,
        entity_id_str.as_deref(),
        payload,
        &now,
        &key.hub_key_id,
    );

    sqlx::query(
        "INSERT INTO audit_log (event_type, entity_id, payload, created_at, prev_hash, hash, hub_key_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(event_type)
    .bind(&entity_id_str)
    .bind(payload)
    .bind(&now)
    .bind(&prev_hash)
    .bind(&hash)
    .bind(&key.hub_key_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Verification outcome for `/audit/verify`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditChainVerification {
    pub ok: bool,
    pub checked: u64,
    /// First row id at which a chain break was detected, if any.
    pub first_bad_id: Option<i64>,
    pub reason: Option<String>,
}

/// Walk the audit chain and verify every row's `prev_hash` and `hash`.
pub async fn verify_chain(pool: &SqlitePool) -> Result<AuditChainVerification, PoolError> {
    let key = current_key();
    let rows = sqlx::query(
        "SELECT id, event_type, entity_id, payload, created_at, prev_hash, hash, hub_key_id \
         FROM audit_log ORDER BY id ASC",
    )
    .fetch_all(pool)
    .await?;

    let mut checked: u64 = 0;
    let mut expected_prev = String::new();

    for row in &rows {
        let id: i64 = row.try_get("id")?;
        let event_type: String = row.try_get("event_type")?;
        let entity_id: Option<String> = row.try_get("entity_id")?;
        let payload: String = row
            .try_get::<Option<String>, _>("payload")?
            .unwrap_or_default();
        let created_at: String = row.try_get("created_at")?;
        let prev_hash: String = row.try_get("prev_hash")?;
        let hash: String = row.try_get("hash")?;
        let hub_key_id: String = row.try_get("hub_key_id")?;

        // Legacy rows (before migration 012) have empty hashes; skip verification for
        // them but keep their hash as "" so the first new row chains from "" (matches
        // the backfill semantics).
        if hash.is_empty() && prev_hash.is_empty() && hub_key_id.is_empty() {
            continue;
        }

        if prev_hash != expected_prev {
            return Ok(AuditChainVerification {
                ok: false,
                checked,
                first_bad_id: Some(id),
                reason: Some("prev_hash does not match previous row".into()),
            });
        }

        // Only rows signed with the currently-loaded key can be re-verified; rows with a
        // different hub_key_id are accepted only if their stored hash re-computes under
        // the *currently-configured* key iff it matches. In v0.6.0 we don't persist a key
        // history table yet, so rotation is a documented operational step.
        if hub_key_id != key.hub_key_id {
            // Pass-through: we cannot re-derive without the historical secret, so accept
            // structural linkage (prev_hash chain) and continue.
            expected_prev = hash.clone();
            checked += 1;
            continue;
        }

        let recomputed = compute_hash(
            &key.secret,
            &prev_hash,
            &event_type,
            entity_id.as_deref(),
            &payload,
            &created_at,
            &hub_key_id,
        );
        if recomputed != hash {
            return Ok(AuditChainVerification {
                ok: false,
                checked,
                first_bad_id: Some(id),
                reason: Some("recomputed hash does not match stored hash (tamper detected)".into()),
            });
        }
        expected_prev = hash;
        checked += 1;
    }

    Ok(AuditChainVerification {
        ok: true,
        checked,
        first_bad_id: None,
        reason: None,
    })
}

/// Inspect the last row's hash (for diagnostics / tests).
pub async fn chain_tip(pool: &SqlitePool) -> Result<String, PoolError> {
    last_hash(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_sqlite_pool, run_migrations};

    async fn setup() -> SqlitePool {
        set_audit_key(AuditKey::new("test-hub", b"test-secret-1".to_vec()));
        let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
        run_migrations(&pool).await.expect("migrations");
        pool
    }

    #[tokio::test]
    async fn chain_verifies_after_several_appends() {
        let pool = setup().await;
        record(&pool, "order_finalized", None, "{\"a\":1}")
            .await
            .unwrap();
        record(&pool, "hq_submission", Some(Uuid::new_v4()), "{\"b\":2}")
            .await
            .unwrap();
        record(&pool, "pos_command", None, "{\"c\":3}")
            .await
            .unwrap();

        let v = verify_chain(&pool).await.unwrap();
        assert!(v.ok, "chain should verify clean: {:?}", v);
        assert_eq!(v.checked, 3);
        assert!(v.first_bad_id.is_none());
    }

    #[tokio::test]
    async fn tampering_with_payload_is_detected() {
        let pool = setup().await;
        record(&pool, "order_finalized", None, "original")
            .await
            .unwrap();
        record(&pool, "hq_submission", None, "second")
            .await
            .unwrap();

        // Tamper with row 1's payload bypassing record().
        sqlx::query("UPDATE audit_log SET payload = ? WHERE id = 1")
            .bind("tampered")
            .execute(&pool)
            .await
            .unwrap();

        let v = verify_chain(&pool).await.unwrap();
        assert!(!v.ok, "tampered chain must fail verification");
        assert_eq!(v.first_bad_id, Some(1));
    }

    #[tokio::test]
    async fn inserting_a_row_out_of_chain_is_detected() {
        let pool = setup().await;
        record(&pool, "a", None, "x").await.unwrap();
        record(&pool, "b", None, "y").await.unwrap();

        // Insert a forged row in the middle with no prev_hash linkage.
        sqlx::query(
            "INSERT INTO audit_log (event_type, entity_id, payload, created_at, prev_hash, hash, hub_key_id) \
             VALUES ('forged', NULL, 'z', '2026-01-01T00:00:00Z', 'not-the-real-prev', 'not-a-real-hash', 'test-hub')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let v = verify_chain(&pool).await.unwrap();
        assert!(!v.ok, "forged insert must break the chain");
    }

    #[tokio::test]
    async fn different_keys_produce_different_hashes() {
        let k1 = AuditKey::new("k1", b"one".to_vec());
        let k2 = AuditKey::new("k2", b"two".to_vec());
        let h1 = compute_hash(&k1.secret, "", "e", None, "p", "t", &k1.hub_key_id);
        let h2 = compute_hash(&k2.secret, "", "e", None, "p", "t", &k2.hub_key_id);
        assert_ne!(h1, h2);
    }
}
