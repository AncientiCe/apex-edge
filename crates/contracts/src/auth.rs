//! Auth contracts for device pairing and session exchange.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCreatePairingCodeRequest {
    pub store_id: Uuid,
    pub created_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCreatePairingCodeResponse {
    pub pairing_code_id: Uuid,
    pub code: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthDevicePairRequest {
    pub pairing_code: String,
    pub store_id: Uuid,
    pub device_name: String,
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthDevicePairResponse {
    pub device_id: Uuid,
    pub device_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSessionExchangeRequest {
    pub external_token: String,
    pub device_id: Uuid,
    pub device_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSessionRefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSessionExchangeResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub refresh_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSessionRevokeResponse {
    pub revoked: bool,
}
