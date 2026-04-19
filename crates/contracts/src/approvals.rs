//! Supervisor approvals: request/grant/deny, plus the `Pending` response payload.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStateDto {
    Pending,
    Granted,
    Denied,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestApprovalPayload {
    pub store_id: Uuid,
    pub register_id: Option<Uuid>,
    pub action: String,
    pub context: serde_json::Value,
    pub requested_by: Option<String>,
    pub ttl_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantApprovalPayload {
    pub approval_id: Uuid,
    pub approver_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenyApprovalPayload {
    pub approval_id: Uuid,
    pub approver_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub approval_id: Uuid,
    pub action: String,
    pub state: ApprovalStateDto,
    pub requested_by: Option<String>,
    pub approver_id: Option<String>,
    pub decision_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}

/// Response envelope when a POS command is gated on supervisor approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPending {
    pub approval_id: Uuid,
    pub action: String,
    pub expires_at: DateTime<Utc>,
}
