//! Print job and result contracts (internal + device adapters).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::DocumentType;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrintJobStatus {
    Queued,
    Rendering,
    Sent,
    Acknowledged,
    Failed,
    ReprintEligible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintJob {
    pub id: Uuid,
    pub document_type: DocumentType,
    pub order_id: Option<Uuid>,
    pub cart_id: Option<Uuid>,
    pub status: PrintJobStatus,
    pub template_id: Uuid,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintJobResult {
    pub job_id: Uuid,
    pub status: PrintJobStatus,
    pub completed_at: DateTime<Utc>,
    pub error_message: Option<String>,
}
