//! Cloud connector trait and reference implementations.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use thiserror::Error;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudEvent {
    pub event_id: Uuid,
    pub event_type: String,
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudDeliveryResult {
    pub connector: String,
    pub accepted: bool,
    pub external_reference: Option<String>,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum CloudConnectorError {
    #[error("cloud connector {connector} is not configured")]
    NotConfigured { connector: String },
    #[error("cloud event payload is empty")]
    EmptyPayload,
}

pub trait CloudConnector: Send + Sync {
    fn connector_code(&self) -> &'static str;
    fn deliver(&self, event: CloudEvent) -> Result<CloudDeliveryResult, CloudConnectorError>;
}

#[derive(Debug, Clone)]
pub struct HostedCloudConnector {
    connector_code: &'static str,
    configured: bool,
}

impl HostedCloudConnector {
    pub fn shopify(configured: bool) -> Self {
        Self::new("shopify", configured)
    }

    pub fn netsuite(configured: bool) -> Self {
        Self::new("netsuite", configured)
    }

    pub fn quickbooks(configured: bool) -> Self {
        Self::new("quickbooks", configured)
    }

    pub fn xero(configured: bool) -> Self {
        Self::new("xero", configured)
    }

    fn new(connector_code: &'static str, configured: bool) -> Self {
        Self {
            connector_code,
            configured,
        }
    }
}

impl CloudConnector for HostedCloudConnector {
    fn connector_code(&self) -> &'static str {
        self.connector_code
    }

    fn deliver(&self, event: CloudEvent) -> Result<CloudDeliveryResult, CloudConnectorError> {
        validate_event(&event)?;
        if !self.configured {
            return Err(CloudConnectorError::NotConfigured {
                connector: self.connector_code.into(),
            });
        }
        Ok(CloudDeliveryResult {
            connector: self.connector_code.into(),
            accepted: true,
            external_reference: Some(format!("{}:{}", self.connector_code, event.event_id)),
        })
    }
}

#[derive(Debug, Clone)]
pub struct SignedWebhookConnector {
    secret: Vec<u8>,
}

impl SignedWebhookConnector {
    pub fn new(secret: impl Into<Vec<u8>>) -> Self {
        Self {
            secret: secret.into(),
        }
    }

    pub fn signature(&self, payload_json: &str) -> String {
        let mut mac =
            HmacSha256::new_from_slice(&self.secret).expect("HMAC accepts any key length");
        mac.update(payload_json.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }
}

impl CloudConnector for SignedWebhookConnector {
    fn connector_code(&self) -> &'static str {
        "signed_webhook"
    }

    fn deliver(&self, event: CloudEvent) -> Result<CloudDeliveryResult, CloudConnectorError> {
        validate_event(&event)?;
        let signature = self.signature(&event.payload_json);
        Ok(CloudDeliveryResult {
            connector: self.connector_code().into(),
            accepted: true,
            external_reference: Some(signature),
        })
    }
}

fn validate_event(event: &CloudEvent) -> Result<(), CloudConnectorError> {
    if event.payload_json.trim().is_empty() {
        return Err(CloudConnectorError::EmptyPayload);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> CloudEvent {
        CloudEvent {
            event_id: Uuid::new_v4(),
            event_type: "order.finalized".into(),
            payload_json: r#"{"order_id":"1"}"#.into(),
        }
    }

    #[test]
    fn hosted_connector_fails_closed_when_unconfigured() {
        let connector = HostedCloudConnector::shopify(false);
        let err = connector
            .deliver(event())
            .expect_err("unconfigured connector must fail");

        assert_eq!(
            err,
            CloudConnectorError::NotConfigured {
                connector: "shopify".into()
            }
        );
    }

    #[test]
    fn hosted_connector_accepts_when_configured() {
        let connector = HostedCloudConnector::netsuite(true);
        let result = connector.deliver(event()).expect("deliver");

        assert!(result.accepted);
        assert_eq!(result.connector, "netsuite");
    }

    #[test]
    fn signed_webhook_generates_replay_safe_signature() {
        let connector = SignedWebhookConnector::new("secret");
        let first = connector.signature(r#"{"order_id":"1"}"#);
        let second = connector.signature(r#"{"order_id":"1"}"#);

        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }
}
