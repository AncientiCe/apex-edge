//! Payment provider adapter trait and reference implementations.
//!
//! ApexEdge never handles raw card data or EMV kernels. Providers return opaque
//! payment ids and receipt metadata that can be stored, printed, and submitted
//! to cloud systems.
//!
//! # Why authorize and capture are separate
//!
//! A store hub can crash, lose power, or lose its network between taking a card and
//! writing the order to its ledger. Authorizing first and capturing only once the sale
//! is safe to complete keeps the failure window as small as possible, and gives the hub
//! a reversal path ([`PaymentProvider::void`]) for the window that remains.
//!
//! # Failure classification
//!
//! [`PaymentProviderError::requires_reversal_check`] is the important distinction: some
//! failures prove no money moved, and some leave it unknown. The latter must never be
//! treated as a clean failure — see the reversal sweeper in `apex-edge-api`.

mod cash;
mod simulated;
mod stripe_terminal;

pub use cash::CashPaymentProvider;
pub use simulated::{
    SimulatedTerminalProvider, SIMULATED_DECLINE_SUFFIX, SIMULATED_INDETERMINATE_SUFFIX,
    SIMULATED_PARTIAL_SUFFIX, SIMULATED_TIMEOUT_SUFFIX,
};
pub use stripe_terminal::{StripeTerminalConfig, StripeTerminalProvider};

use apex_edge_contracts::PaymentProviderReceipt;
use async_trait::async_trait;
use thiserror::Error;
use uuid::Uuid;

/// Request to authorize a payment on a terminal or tender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeRequest {
    /// Carried through from the POS envelope so a retried command reuses the same
    /// provider-side authorization instead of charging twice.
    pub idempotency_key: Uuid,
    pub cart_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    /// Full amount to charge, tip included.
    pub amount_cents: u64,
    /// How much of `amount_cents` is tip. Informational: providers charge `amount_cents`.
    pub tip_amount_cents: u64,
    pub currency: String,
}

/// What the provider decided. A decline is a normal outcome, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationOutcome {
    Approved,
    /// The card approved less than was asked for; the POS must collect the remainder
    /// on another tender.
    PartiallyApproved {
        requested_cents: u64,
    },
    Declined {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentAuthorization {
    pub provider: String,
    pub provider_payment_id: String,
    /// Zero when declined.
    pub approved_cents: u64,
    pub tip_amount_cents: u64,
    pub outcome: AuthorizationOutcome,
    pub receipt: Option<PaymentProviderReceipt>,
}

impl PaymentAuthorization {
    pub fn is_approved(&self) -> bool {
        matches!(
            self.outcome,
            AuthorizationOutcome::Approved | AuthorizationOutcome::PartiallyApproved { .. }
        )
    }

    /// Decline code and message, when the provider declined.
    pub fn declined_reason(&self) -> Option<(&str, &str)> {
        match &self.outcome {
            AuthorizationOutcome::Declined { code, message } => {
                Some((code.as_str(), message.as_str()))
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRequest {
    pub idempotency_key: Uuid,
    pub provider_payment_id: String,
    pub amount_cents: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentCapture {
    pub provider: String,
    pub provider_payment_id: String,
    pub captured_cents: u64,
    pub receipt: PaymentProviderReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoidRequest {
    pub idempotency_key: Uuid,
    pub provider_payment_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundRequest {
    pub idempotency_key: Uuid,
    pub provider_payment_id: String,
    pub amount_cents: u64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentRefund {
    pub provider: String,
    pub provider_refund_id: String,
    pub refunded_cents: u64,
    pub receipt: PaymentProviderReceipt,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PaymentProviderError {
    #[error("payment amount must be greater than zero")]
    InvalidAmount,
    #[error("payment provider {provider} is not configured")]
    NotConfigured { provider: String },
    #[error("payment {provider_payment_id} was not found")]
    PaymentNotFound { provider_payment_id: String },
    #[error("payment provider {provider} did not respond in time")]
    Timeout { provider: String },
    #[error("payment provider {provider} transport failure: {message}")]
    Transport { provider: String, message: String },
    /// The provider may or may not have moved money. Treating this as a plain failure
    /// risks completing a sale that was paid for, or abandoning one that was.
    #[error("payment provider {provider} returned an indeterminate result: {message}")]
    Indeterminate {
        provider: String,
        provider_payment_id: Option<String>,
        message: String,
    },
}

impl PaymentProviderError {
    /// True when money may have moved despite the error, so a reversal check is owed.
    pub fn requires_reversal_check(&self) -> bool {
        match self {
            PaymentProviderError::Indeterminate { .. } => true,
            PaymentProviderError::Transport { .. } => true,
            PaymentProviderError::InvalidAmount
            | PaymentProviderError::NotConfigured { .. }
            | PaymentProviderError::PaymentNotFound { .. }
            | PaymentProviderError::Timeout { .. } => false,
        }
    }
}

#[async_trait]
pub trait PaymentProvider: Send + Sync {
    fn provider_code(&self) -> &'static str;

    /// Hold funds. Returns a decline as an `Ok` outcome; reserve `Err` for failures
    /// where the provider could not give an answer.
    async fn authorize(
        &self,
        request: AuthorizeRequest,
    ) -> Result<PaymentAuthorization, PaymentProviderError>;

    /// Commit previously authorized funds.
    async fn capture(
        &self,
        request: CaptureRequest,
    ) -> Result<PaymentCapture, PaymentProviderError>;

    /// Release an authorization or reverse a same-day capture.
    async fn void(&self, request: VoidRequest) -> Result<(), PaymentProviderError>;

    /// Return funds after settlement.
    async fn refund(&self, request: RefundRequest) -> Result<PaymentRefund, PaymentProviderError>;
}

pub(crate) fn validate_amount(amount_cents: u64) -> Result<(), PaymentProviderError> {
    if amount_cents == 0 {
        return Err(PaymentProviderError::InvalidAmount);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approved(amount: u64) -> PaymentAuthorization {
        PaymentAuthorization {
            provider: "test".into(),
            provider_payment_id: "p_1".into(),
            approved_cents: amount,
            tip_amount_cents: 0,
            outcome: AuthorizationOutcome::Approved,
            receipt: None,
        }
    }

    #[test]
    fn partial_approval_still_counts_as_approved() {
        let mut auth = approved(500);
        auth.outcome = AuthorizationOutcome::PartiallyApproved {
            requested_cents: 1_000,
        };
        assert!(auth.is_approved());
        assert!(auth.declined_reason().is_none());
    }

    #[test]
    fn decline_is_not_approved_and_carries_a_reason() {
        let mut auth = approved(0);
        auth.outcome = AuthorizationOutcome::Declined {
            code: "card_declined".into(),
            message: "The card was declined.".into(),
        };
        assert!(!auth.is_approved());
        assert_eq!(
            auth.declined_reason().map(|(c, _)| c),
            Some("card_declined")
        );
    }

    #[test]
    fn only_ambiguous_failures_owe_a_reversal_check() {
        assert!(!PaymentProviderError::InvalidAmount.requires_reversal_check());
        assert!(!PaymentProviderError::Timeout {
            provider: "x".into()
        }
        .requires_reversal_check());
        assert!(PaymentProviderError::Transport {
            provider: "x".into(),
            message: "connection reset".into()
        }
        .requires_reversal_check());
        assert!(PaymentProviderError::Indeterminate {
            provider: "x".into(),
            provider_payment_id: None,
            message: "no response".into()
        }
        .requires_reversal_check());
    }
}
