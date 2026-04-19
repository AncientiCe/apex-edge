//! Supervisor approvals persistence.
//!
//! Approvals are created implicitly by commands that need supervisor sign-off (blind
//! return, large manual discount, paid-out over threshold, cash variance, void after
//! tender). Commands return `Pending { approval_id }`; the POS waits on `/pos/stream`
//! for `approval_granted` or calls `grant_approval`.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Pending,
    Granted,
    Denied,
    Expired,
}

impl ApprovalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Granted => "granted",
            Self::Denied => "denied",
            Self::Expired => "expired",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "granted" => Some(Self::Granted),
            "denied" => Some(Self::Denied),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub id: Uuid,
    pub store_id: Uuid,
    pub register_id: Option<Uuid>,
    pub action: String,
    pub requested_by: Option<String>,
    pub context_json: String,
    pub state: ApprovalState,
    pub approver_id: Option<String>,
    pub decision_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}

fn row_to_record(row: &sqlx::sqlite::SqliteRow) -> Result<ApprovalRecord, PoolError> {
    let id: String = row.try_get("id")?;
    let store_id: String = row.try_get("store_id")?;
    let register_id: Option<String> = row.try_get("register_id")?;
    let state_str: String = row.try_get("state")?;
    let created_at: String = row.try_get("created_at")?;
    let decided_at: Option<String> = row.try_get("decided_at")?;
    let expires_at: String = row.try_get("expires_at")?;
    Ok(ApprovalRecord {
        id: Uuid::parse_str(&id).map_err(|_| PoolError::Other("bad uuid".into()))?,
        store_id: Uuid::parse_str(&store_id).map_err(|_| PoolError::Other("bad uuid".into()))?,
        register_id: register_id.as_deref().and_then(|s| Uuid::parse_str(s).ok()),
        action: row.try_get("action")?,
        requested_by: row.try_get("requested_by")?,
        context_json: row.try_get("context")?,
        state: ApprovalState::parse(&state_str).unwrap_or(ApprovalState::Pending),
        approver_id: row.try_get("approver_id")?,
        decision_reason: row.try_get("decision_reason")?,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        decided_at: decided_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc)),
        expires_at: DateTime::parse_from_rfc3339(&expires_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}

pub async fn request_approval(
    pool: &SqlitePool,
    store_id: Uuid,
    register_id: Option<Uuid>,
    action: &str,
    requested_by: Option<&str>,
    context_json: &str,
    ttl_seconds: i64,
) -> Result<ApprovalRecord, PoolError> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let expires = now + Duration::seconds(ttl_seconds.max(10));
    sqlx::query(
        "INSERT INTO approvals (id, store_id, register_id, action, requested_by, context, state, created_at, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(register_id.map(|u| u.to_string()))
    .bind(action)
    .bind(requested_by)
    .bind(context_json)
    .bind(now.to_rfc3339())
    .bind(expires.to_rfc3339())
    .execute(pool)
    .await?;
    fetch_approval(pool, id)
        .await?
        .ok_or_else(|| PoolError::Other("inserted approval not found".into()))
}

pub async fn fetch_approval(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<Option<ApprovalRecord>, PoolError> {
    let row = sqlx::query("SELECT * FROM approvals WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    match row {
        Some(r) => Ok(Some(row_to_record(&r)?)),
        None => Ok(None),
    }
}

pub async fn grant_approval(
    pool: &SqlitePool,
    id: Uuid,
    approver_id: Option<&str>,
    reason: Option<&str>,
) -> Result<ApprovalRecord, PoolError> {
    let now = Utc::now();
    let existing = fetch_approval(pool, id)
        .await?
        .ok_or_else(|| PoolError::Other("approval not found".into()))?;
    if existing.state != ApprovalState::Pending {
        return Ok(existing);
    }
    if existing.expires_at < now {
        sqlx::query("UPDATE approvals SET state = 'expired', decided_at = ? WHERE id = ?")
            .bind(now.to_rfc3339())
            .bind(id.to_string())
            .execute(pool)
            .await?;
        return Ok(fetch_approval(pool, id).await?.unwrap_or(existing));
    }
    sqlx::query(
        "UPDATE approvals SET state = 'granted', approver_id = ?, decision_reason = ?, decided_at = ? \
         WHERE id = ? AND state = 'pending'",
    )
    .bind(approver_id)
    .bind(reason)
    .bind(now.to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(fetch_approval(pool, id).await?.unwrap_or(existing))
}

pub async fn deny_approval(
    pool: &SqlitePool,
    id: Uuid,
    approver_id: Option<&str>,
    reason: Option<&str>,
) -> Result<ApprovalRecord, PoolError> {
    let now = Utc::now();
    sqlx::query(
        "UPDATE approvals SET state = 'denied', approver_id = ?, decision_reason = ?, decided_at = ? \
         WHERE id = ? AND state = 'pending'",
    )
    .bind(approver_id)
    .bind(reason)
    .bind(now.to_rfc3339())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    fetch_approval(pool, id)
        .await?
        .ok_or_else(|| PoolError::Other("approval not found".into()))
}

pub async fn expire_pending_approvals(pool: &SqlitePool) -> Result<u64, PoolError> {
    let now = Utc::now();
    let result = sqlx::query(
        "UPDATE approvals SET state = 'expired', decided_at = ? \
         WHERE state = 'pending' AND expires_at < ?",
    )
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
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
    async fn request_then_grant_flows_state() {
        let pool = setup().await;
        let approval = request_approval(
            &pool,
            Uuid::new_v4(),
            Some(Uuid::new_v4()),
            "blind_return",
            Some("associate-1"),
            "{\"amount_cents\":5000}",
            300,
        )
        .await
        .unwrap();
        assert_eq!(approval.state, ApprovalState::Pending);

        let granted = grant_approval(&pool, approval.id, Some("mgr-1"), Some("ok"))
            .await
            .unwrap();
        assert_eq!(granted.state, ApprovalState::Granted);
        assert_eq!(granted.approver_id.as_deref(), Some("mgr-1"));
    }

    #[tokio::test]
    async fn deny_sets_denied_state() {
        let pool = setup().await;
        let approval = request_approval(
            &pool,
            Uuid::new_v4(),
            None,
            "manual_discount",
            None,
            "{}",
            300,
        )
        .await
        .unwrap();
        let denied = deny_approval(&pool, approval.id, Some("mgr-1"), Some("too large"))
            .await
            .unwrap();
        assert_eq!(denied.state, ApprovalState::Denied);
    }

    #[tokio::test]
    async fn expired_pending_transitions_when_ttl_elapses() {
        let pool = setup().await;
        let approval = request_approval(&pool, Uuid::new_v4(), None, "paid_out", None, "{}", 10)
            .await
            .unwrap();
        // Force expires_at into the past.
        sqlx::query("UPDATE approvals SET expires_at = '2000-01-01T00:00:00Z' WHERE id = ?")
            .bind(approval.id.to_string())
            .execute(&pool)
            .await
            .unwrap();
        let n = expire_pending_approvals(&pool).await.unwrap();
        assert_eq!(n, 1);
        let reloaded = fetch_approval(&pool, approval.id).await.unwrap().unwrap();
        assert_eq!(reloaded.state, ApprovalState::Expired);
    }
}
