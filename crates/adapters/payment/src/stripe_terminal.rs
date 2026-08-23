//! Stripe Terminal, server-driven.
//!
//! This talks to the Stripe REST API directly rather than a Terminal SDK, which is the
//! recommended integration for WisePOS E and Stripe Reader S700/S710 and means the whole
//! flow works from a Rust service with no native dependencies.
//!
//! It also means the integration is verifiable with no hardware: register a reader with
//! registration code `simulated-wpe` in a sandbox and drive card presentment with
//! `POST /v1/test_helpers/terminal/readers/{id}/present_payment_method`.
//!
//! Flow: create a manual-capture PaymentIntent, hand it to the reader, poll the reader
//! action until it resolves, then capture once the sale is safe to complete.

use std::time::Duration;

use apex_edge_contracts::{PaymentEntryMethod, PaymentProviderReceipt};
use async_trait::async_trait;
use reqwest::{Client, Response};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    validate_amount, AuthorizationOutcome, AuthorizeRequest, CaptureRequest, PaymentAuthorization,
    PaymentCapture, PaymentProvider, PaymentProviderError, PaymentRefund, RefundRequest,
    VoidRequest,
};

const PROVIDER_CODE: &str = "stripe_terminal";

#[derive(Debug, Clone)]
pub struct StripeTerminalConfig {
    pub secret_key: String,
    /// Terminal reader to drive. In a sandbox this can be a `simulated-wpe` reader.
    pub reader_id: String,
    /// Overridable so tests can point at a local stand-in for the Stripe API.
    pub api_base_url: String,
    pub poll_interval: Duration,
    pub poll_timeout: Duration,
}

impl Default for StripeTerminalConfig {
    fn default() -> Self {
        Self {
            secret_key: String::new(),
            reader_id: String::new(),
            api_base_url: "https://api.stripe.com".into(),
            poll_interval: Duration::from_millis(500),
            poll_timeout: Duration::from_secs(90),
        }
    }
}

pub struct StripeTerminalProvider {
    config: StripeTerminalConfig,
    client: Client,
}

impl std::fmt::Debug for StripeTerminalProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeTerminalProvider")
            .field("reader_id", &self.config.reader_id)
            .field("api_base_url", &self.config.api_base_url)
            .finish()
    }
}

impl StripeTerminalProvider {
    /// Fails closed when credentials or the reader are missing, so a misconfigured hub
    /// cannot silently fall back to recording unpaid tenders.
    pub fn new(config: StripeTerminalConfig) -> Result<Self, PaymentProviderError> {
        if config.secret_key.trim().is_empty() || config.reader_id.trim().is_empty() {
            return Err(PaymentProviderError::NotConfigured {
                provider: PROVIDER_CODE.into(),
            });
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| PaymentProviderError::Transport {
                provider: PROVIDER_CODE.into(),
                message: e.to_string(),
            })?;
        Ok(Self { config, client })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.api_base_url.trim_end_matches('/'))
    }

    async fn post_form(
        &self,
        path: &str,
        idempotency_key: Uuid,
        form: &[(&str, String)],
    ) -> Result<Value, PaymentProviderError> {
        let response = self
            .client
            .post(self.url(path))
            .basic_auth(&self.config.secret_key, Some(""))
            .header("Idempotency-Key", idempotency_key.to_string())
            .form(form)
            .send()
            .await
            .map_err(|e| self.transport_error(e))?;
        self.parse(response).await
    }

    async fn get(&self, path: &str) -> Result<Value, PaymentProviderError> {
        let response = self
            .client
            .get(self.url(path))
            .basic_auth(&self.config.secret_key, Some(""))
            .send()
            .await
            .map_err(|e| self.transport_error(e))?;
        self.parse(response).await
    }

    fn transport_error(&self, error: reqwest::Error) -> PaymentProviderError {
        if error.is_timeout() {
            return PaymentProviderError::Indeterminate {
                provider: PROVIDER_CODE.into(),
                provider_payment_id: None,
                message: format!("request timed out: {error}"),
            };
        }
        PaymentProviderError::Transport {
            provider: PROVIDER_CODE.into(),
            message: error.to_string(),
        }
    }

    async fn parse(&self, response: Response) -> Result<Value, PaymentProviderError> {
        let status = response.status();
        let body = response.text().await.map_err(|e| self.transport_error(e))?;

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(PaymentProviderError::PaymentNotFound {
                provider_payment_id: String::new(),
            });
        }
        if !status.is_success() {
            return Err(PaymentProviderError::Indeterminate {
                provider: PROVIDER_CODE.into(),
                provider_payment_id: None,
                message: format!("stripe returned {status}: {body}"),
            });
        }
        serde_json::from_str(&body).map_err(|e| PaymentProviderError::Indeterminate {
            provider: PROVIDER_CODE.into(),
            provider_payment_id: None,
            message: format!("unreadable stripe response: {e}"),
        })
    }

    /// Poll the reader until its action resolves. A timeout here is indeterminate, not a
    /// clean failure: the card may already have been presented.
    async fn await_reader_action(
        &self,
        payment_intent_id: &str,
    ) -> Result<ReaderAction, PaymentProviderError> {
        let path = format!("/v1/terminal/readers/{}", self.config.reader_id);
        let deadline = tokio::time::Instant::now() + self.config.poll_timeout;

        loop {
            let reader = self.get(&path).await?;
            let action = reader.get("action");
            let status = action
                .and_then(|a| a.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("in_progress");

            match status {
                "succeeded" => return Ok(ReaderAction::Succeeded),
                "failed" => {
                    let code = action
                        .and_then(|a| a.get("failure_code"))
                        .and_then(Value::as_str)
                        .unwrap_or("reader_action_failed")
                        .to_string();
                    let message = action
                        .and_then(|a| a.get("failure_message"))
                        .and_then(Value::as_str)
                        .unwrap_or("The reader could not complete the payment.")
                        .to_string();
                    return Ok(ReaderAction::Failed { code, message });
                }
                _ => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(PaymentProviderError::Indeterminate {
                            provider: PROVIDER_CODE.into(),
                            provider_payment_id: Some(payment_intent_id.to_string()),
                            message: "reader action did not resolve before the deadline".into(),
                        });
                    }
                    tokio::time::sleep(self.config.poll_interval).await;
                }
            }
        }
    }

    fn receipt_from_intent(intent: &Value, provider_payment_id: &str) -> PaymentProviderReceipt {
        let card_present = intent
            .get("latest_charge")
            .and_then(|c| c.get("payment_method_details"))
            .and_then(|d| d.get("card_present"));

        let last4 = card_present
            .and_then(|c| c.get("last4"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let aid = card_present
            .and_then(|c| c.get("receipt"))
            .and_then(|r| r.get("dedicated_file_name"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let authorization_code = card_present
            .and_then(|c| c.get("receipt"))
            .and_then(|r| r.get("authorization_code"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let entry_method = card_present
            .and_then(|c| c.get("read_method"))
            .and_then(Value::as_str)
            .map(|method| match method {
                "contactless_emv" | "contactless_magstripe_mode" => PaymentEntryMethod::Contactless,
                "magnetic_stripe_track2" | "magnetic_stripe_fallback" => PaymentEntryMethod::Swipe,
                _ => PaymentEntryMethod::Dip,
            })
            .or(Some(PaymentEntryMethod::Contactless));

        PaymentProviderReceipt {
            provider: PROVIDER_CODE.into(),
            provider_payment_id: provider_payment_id.into(),
            entry_method,
            last4,
            aid,
            authorization_code,
        }
    }
}

enum ReaderAction {
    Succeeded,
    Failed { code: String, message: String },
}

#[async_trait]
impl PaymentProvider for StripeTerminalProvider {
    fn provider_code(&self) -> &'static str {
        PROVIDER_CODE
    }

    async fn authorize(
        &self,
        request: AuthorizeRequest,
    ) -> Result<PaymentAuthorization, PaymentProviderError> {
        validate_amount(request.amount_cents)?;

        let intent = self
            .post_form(
                "/v1/payment_intents",
                request.idempotency_key,
                &[
                    ("amount", request.amount_cents.to_string()),
                    ("currency", request.currency.to_lowercase()),
                    ("payment_method_types[]", "card_present".into()),
                    // Manual capture keeps the window between taking a card and having a
                    // durable order as short as possible, and leaves a reversal path.
                    ("capture_method", "manual".into()),
                    ("metadata[cart_id]", request.cart_id.to_string()),
                    ("metadata[store_id]", request.store_id.to_string()),
                    ("metadata[register_id]", request.register_id.to_string()),
                    ("metadata[tip_cents]", request.tip_amount_cents.to_string()),
                ],
            )
            .await?;

        let payment_intent_id = intent
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| PaymentProviderError::Indeterminate {
                provider: PROVIDER_CODE.into(),
                provider_payment_id: None,
                message: "stripe did not return a payment intent id".into(),
            })?
            .to_string();

        self.post_form(
            &format!(
                "/v1/terminal/readers/{}/process_payment_intent",
                self.config.reader_id
            ),
            request.idempotency_key,
            &[("payment_intent", payment_intent_id.clone())],
        )
        .await?;

        match self.await_reader_action(&payment_intent_id).await? {
            ReaderAction::Failed { code, message } => Ok(PaymentAuthorization {
                provider: PROVIDER_CODE.into(),
                provider_payment_id: payment_intent_id,
                approved_cents: 0,
                tip_amount_cents: 0,
                outcome: AuthorizationOutcome::Declined { code, message },
                receipt: None,
            }),
            ReaderAction::Succeeded => {
                let confirmed = self
                    .get(&format!(
                        "/v1/payment_intents/{payment_intent_id}?expand[]=latest_charge"
                    ))
                    .await?;

                let approved_cents = confirmed
                    .get("amount")
                    .and_then(Value::as_u64)
                    .unwrap_or(request.amount_cents);
                let outcome = if approved_cents < request.amount_cents {
                    AuthorizationOutcome::PartiallyApproved {
                        requested_cents: request.amount_cents,
                    }
                } else {
                    AuthorizationOutcome::Approved
                };

                Ok(PaymentAuthorization {
                    provider: PROVIDER_CODE.into(),
                    provider_payment_id: payment_intent_id.clone(),
                    approved_cents,
                    tip_amount_cents: request.tip_amount_cents,
                    outcome,
                    receipt: Some(Self::receipt_from_intent(&confirmed, &payment_intent_id)),
                })
            }
        }
    }

    async fn capture(
        &self,
        request: CaptureRequest,
    ) -> Result<PaymentCapture, PaymentProviderError> {
        validate_amount(request.amount_cents)?;
        let captured = self
            .post_form(
                &format!(
                    "/v1/payment_intents/{}/capture",
                    request.provider_payment_id
                ),
                request.idempotency_key,
                &[("amount_to_capture", request.amount_cents.to_string())],
            )
            .await?;

        let captured_cents = captured
            .get("amount_received")
            .and_then(Value::as_u64)
            .unwrap_or(request.amount_cents);

        Ok(PaymentCapture {
            provider: PROVIDER_CODE.into(),
            provider_payment_id: request.provider_payment_id.clone(),
            captured_cents,
            receipt: Self::receipt_from_intent(&captured, &request.provider_payment_id),
        })
    }

    async fn void(&self, request: VoidRequest) -> Result<(), PaymentProviderError> {
        let mut form = Vec::new();
        if let Some(reason) = request.reason.as_deref() {
            form.push(("metadata[reversal_reason]", reason.to_string()));
        }
        self.post_form(
            &format!("/v1/payment_intents/{}/cancel", request.provider_payment_id),
            request.idempotency_key,
            &form,
        )
        .await?;
        Ok(())
    }

    async fn refund(&self, request: RefundRequest) -> Result<PaymentRefund, PaymentProviderError> {
        validate_amount(request.amount_cents)?;
        let mut form = vec![
            ("payment_intent", request.provider_payment_id.clone()),
            ("amount", request.amount_cents.to_string()),
        ];
        if let Some(reason) = request.reason.as_deref() {
            form.push(("metadata[reason]", reason.to_string()));
        }
        let refund = self
            .post_form("/v1/refunds", request.idempotency_key, &form)
            .await?;

        let provider_refund_id = refund
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| PaymentProviderError::Indeterminate {
                provider: PROVIDER_CODE.into(),
                provider_payment_id: Some(request.provider_payment_id.clone()),
                message: "stripe did not return a refund id".into(),
            })?
            .to_string();

        Ok(PaymentRefund {
            provider: PROVIDER_CODE.into(),
            provider_refund_id,
            refunded_cents: refund
                .get("amount")
                .and_then(Value::as_u64)
                .unwrap_or(request.amount_cents),
            receipt: Self::receipt_from_intent(&refund, &request.provider_payment_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_credentials_fail_closed() {
        let err = StripeTerminalProvider::new(StripeTerminalConfig {
            reader_id: "tmr_1".into(),
            ..Default::default()
        })
        .expect_err("no secret key");
        assert!(matches!(err, PaymentProviderError::NotConfigured { .. }));

        let err = StripeTerminalProvider::new(StripeTerminalConfig {
            secret_key: "sk_test".into(),
            ..Default::default()
        })
        .expect_err("no reader");
        assert!(matches!(err, PaymentProviderError::NotConfigured { .. }));
    }

    #[test]
    fn card_read_method_maps_to_entry_method() {
        let intent = serde_json::json!({
            "latest_charge": {
                "payment_method_details": {
                    "card_present": { "last4": "1234", "read_method": "magnetic_stripe_track2" }
                }
            }
        });
        let receipt = StripeTerminalProvider::receipt_from_intent(&intent, "pi_1");
        assert_eq!(receipt.entry_method, Some(PaymentEntryMethod::Swipe));
        assert_eq!(receipt.last4.as_deref(), Some("1234"));
    }
}
