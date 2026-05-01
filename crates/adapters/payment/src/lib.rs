//! Payment provider adapter trait and safe reference implementations.
//!
//! ApexEdge never handles raw card data or EMV kernels. Providers return opaque
//! payment ids and receipt metadata that can be stored, printed, and submitted
//! to cloud systems.

use apex_edge_contracts::{PaymentEntryMethod, PaymentProviderReceipt};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentStartRequest {
    pub cart_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub amount_cents: u64,
    pub tip_amount_cents: u64,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentStartResponse {
    pub provider: String,
    pub provider_payment_id: String,
    pub amount_cents: u64,
    pub tip_amount_cents: u64,
    pub entry_method: Option<PaymentEntryMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentRefundRequest {
    pub provider_payment_id: String,
    pub amount_cents: u64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PaymentProviderError {
    #[error("payment amount must be greater than zero")]
    InvalidAmount,
    #[error("payment provider {provider} is not configured")]
    NotConfigured { provider: String },
    #[error("payment {provider_payment_id} was not found")]
    PaymentNotFound { provider_payment_id: String },
}

pub trait PaymentProvider: Send + Sync {
    fn provider_code(&self) -> &'static str;

    fn start_payment(
        &self,
        request: PaymentStartRequest,
    ) -> Result<PaymentStartResponse, PaymentProviderError>;

    fn cancel(&self, provider_payment_id: &str) -> Result<(), PaymentProviderError>;

    fn confirm(
        &self,
        provider_payment_id: &str,
    ) -> Result<PaymentProviderReceipt, PaymentProviderError>;

    fn refund(
        &self,
        request: PaymentRefundRequest,
    ) -> Result<PaymentProviderReceipt, PaymentProviderError>;
}

#[derive(Debug, Clone, Default)]
pub struct CashPaymentProvider;

impl PaymentProvider for CashPaymentProvider {
    fn provider_code(&self) -> &'static str {
        "cash"
    }

    fn start_payment(
        &self,
        request: PaymentStartRequest,
    ) -> Result<PaymentStartResponse, PaymentProviderError> {
        validate_amount(request.amount_cents)?;
        Ok(PaymentStartResponse {
            provider: self.provider_code().into(),
            provider_payment_id: format!("cash_{}", Uuid::new_v4()),
            amount_cents: request.amount_cents,
            tip_amount_cents: request.tip_amount_cents,
            entry_method: Some(PaymentEntryMethod::Cash),
        })
    }

    fn cancel(&self, _provider_payment_id: &str) -> Result<(), PaymentProviderError> {
        Ok(())
    }

    fn confirm(
        &self,
        provider_payment_id: &str,
    ) -> Result<PaymentProviderReceipt, PaymentProviderError> {
        Ok(PaymentProviderReceipt {
            provider: self.provider_code().into(),
            provider_payment_id: provider_payment_id.into(),
            entry_method: Some(PaymentEntryMethod::Cash),
            last4: None,
            aid: None,
            authorization_code: None,
        })
    }

    fn refund(
        &self,
        request: PaymentRefundRequest,
    ) -> Result<PaymentProviderReceipt, PaymentProviderError> {
        validate_amount(request.amount_cents)?;
        self.confirm(&request.provider_payment_id)
    }
}

#[derive(Debug, Clone)]
pub struct HostedTerminalProvider {
    provider_code: &'static str,
    configured: bool,
}

impl HostedTerminalProvider {
    pub fn stripe_terminal(configured: bool) -> Self {
        Self {
            provider_code: "stripe_terminal",
            configured,
        }
    }

    pub fn adyen_terminal(configured: bool) -> Self {
        Self {
            provider_code: "adyen_terminal",
            configured,
        }
    }
}

impl PaymentProvider for HostedTerminalProvider {
    fn provider_code(&self) -> &'static str {
        self.provider_code
    }

    fn start_payment(
        &self,
        request: PaymentStartRequest,
    ) -> Result<PaymentStartResponse, PaymentProviderError> {
        validate_configured(self.provider_code, self.configured)?;
        validate_amount(request.amount_cents)?;
        Ok(PaymentStartResponse {
            provider: self.provider_code.into(),
            provider_payment_id: format!("{}_{}", self.provider_code, Uuid::new_v4()),
            amount_cents: request.amount_cents,
            tip_amount_cents: request.tip_amount_cents,
            entry_method: Some(PaymentEntryMethod::Contactless),
        })
    }

    fn cancel(&self, provider_payment_id: &str) -> Result<(), PaymentProviderError> {
        validate_configured(self.provider_code, self.configured)?;
        validate_payment_id(provider_payment_id)?;
        Ok(())
    }

    fn confirm(
        &self,
        provider_payment_id: &str,
    ) -> Result<PaymentProviderReceipt, PaymentProviderError> {
        validate_configured(self.provider_code, self.configured)?;
        validate_payment_id(provider_payment_id)?;
        Ok(PaymentProviderReceipt {
            provider: self.provider_code.into(),
            provider_payment_id: provider_payment_id.into(),
            entry_method: Some(PaymentEntryMethod::Contactless),
            last4: Some("4242".into()),
            aid: None,
            authorization_code: None,
        })
    }

    fn refund(
        &self,
        request: PaymentRefundRequest,
    ) -> Result<PaymentProviderReceipt, PaymentProviderError> {
        validate_configured(self.provider_code, self.configured)?;
        validate_amount(request.amount_cents)?;
        self.confirm(&request.provider_payment_id)
    }
}

fn validate_amount(amount_cents: u64) -> Result<(), PaymentProviderError> {
    if amount_cents == 0 {
        return Err(PaymentProviderError::InvalidAmount);
    }
    Ok(())
}

fn validate_configured(provider: &str, configured: bool) -> Result<(), PaymentProviderError> {
    if !configured {
        return Err(PaymentProviderError::NotConfigured {
            provider: provider.into(),
        });
    }
    Ok(())
}

fn validate_payment_id(provider_payment_id: &str) -> Result<(), PaymentProviderError> {
    if provider_payment_id.trim().is_empty() {
        return Err(PaymentProviderError::PaymentNotFound {
            provider_payment_id: provider_payment_id.into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(amount_cents: u64) -> PaymentStartRequest {
        PaymentStartRequest {
            cart_id: Uuid::new_v4(),
            store_id: Uuid::new_v4(),
            register_id: Uuid::new_v4(),
            amount_cents,
            tip_amount_cents: 25,
            currency: "USD".into(),
        }
    }

    #[test]
    fn cash_provider_returns_cash_receipt_metadata() {
        let provider = CashPaymentProvider;

        let started = provider
            .start_payment(request(1_000))
            .expect("cash payment starts");
        let receipt = provider
            .confirm(&started.provider_payment_id)
            .expect("cash payment confirms");

        assert_eq!(started.provider, "cash");
        assert_eq!(started.tip_amount_cents, 25);
        assert_eq!(receipt.provider, "cash");
        assert_eq!(receipt.entry_method, Some(PaymentEntryMethod::Cash));
    }

    #[test]
    fn hosted_provider_fails_closed_when_not_configured() {
        let provider = HostedTerminalProvider::stripe_terminal(false);
        let err = provider
            .start_payment(request(1_000))
            .expect_err("unconfigured provider must fail");

        assert_eq!(
            err,
            PaymentProviderError::NotConfigured {
                provider: "stripe_terminal".into()
            }
        );
    }

    #[test]
    fn hosted_provider_returns_terminal_receipt_metadata() {
        let provider = HostedTerminalProvider::adyen_terminal(true);

        let started = provider
            .start_payment(request(1_000))
            .expect("terminal payment starts");
        let receipt = provider
            .confirm(&started.provider_payment_id)
            .expect("terminal payment confirms");

        assert_eq!(started.provider, "adyen_terminal");
        assert_eq!(receipt.provider, "adyen_terminal");
        assert_eq!(receipt.last4.as_deref(), Some("4242"));
        assert_eq!(receipt.entry_method, Some(PaymentEntryMethod::Contactless));
    }
}
