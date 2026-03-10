//! Customer contract (HQ -> ApexEdge sync).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Customer {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub email: Option<String>,
    pub version: u64,
}
