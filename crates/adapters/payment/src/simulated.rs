//! A deterministic terminal for tests, CI, and the POS simulator.
//!
//! Every branch a real terminal can take is reachable without hardware or an account,
//! selected by the last two digits of the amount. This mirrors how Stripe's test amounts
//! work, so the trigger stays visible in the request rather than hidden in configuration.
//!
//! | Amount ends in | Behaviour |
//! |----------------|-----------|
//! | `05` | declined (`card_declined`) |
//! | `11` | authorize times out (no funds moved) |
//! | `13` | partially approved (half, rounded down) |
//! | `17` | indeterminate (a reversal check is owed) |
//! | anything else | approved |

use std::collections::HashMap;
use std::sync::Mutex;

use apex_edge_contracts::{PaymentEntryMethod, PaymentProviderReceipt};
use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    validate_amount, AuthorizationOutcome, AuthorizeRequest, CaptureRequest, PaymentAuthorization,
    PaymentCapture, PaymentProvider, PaymentProviderError, PaymentRefund, RefundRequest,
    VoidRequest,
};

pub const SIMULATED_DECLINE_SUFFIX: u64 = 5;
pub const SIMULATED_TIMEOUT_SUFFIX: u64 = 11;
pub const SIMULATED_PARTIAL_SUFFIX: u64 = 13;
pub const SIMULATED_INDETERMINATE_SUFFIX: u64 = 17;

const PROVIDER_CODE: &str = "simulated_terminal";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaymentState {
    Authorized,
    Captured,
    Reversed,
}

#[derive(Debug, Default)]
struct SimulatedLedger {
    /// Authorizations keyed by idempotency key, so retries are answered from here.
    by_idempotency_key: HashMap<Uuid, PaymentAuthorization>,
    states: HashMap<String, PaymentState>,
}

#[derive(Debug, Default)]
pub struct SimulatedTerminalProvider {
    ledger: Mutex<SimulatedLedger>,
}

impl SimulatedTerminalProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct authorizations performed. Used by tests to prove a retry did
    /// not start a second payment.
    pub fn authorization_count(&self) -> usize {
        self.ledger
            .lock()
            .map(|l| l.by_idempotency_key.len())
            .unwrap_or(0)
    }

    pub fn is_reversed(&self, provider_payment_id: &str) -> bool {
        self.ledger
            .lock()
            .map(|l| l.states.get(provider_payment_id) == Some(&PaymentState::Reversed))
            .unwrap_or(false)
    }

    fn receipt(provider_payment_id: &str) -> PaymentProviderReceipt {
        PaymentProviderReceipt {
            provider: PROVIDER_CODE.into(),
            provider_payment_id: provider_payment_id.into(),
            entry_method: Some(PaymentEntryMethod::Contactless),
            last4: Some("4242".into()),
            aid: Some("A0000000031010".into()),
            authorization_code: Some("SIMAPPROVED".into()),
        }
    }

    fn transition(
        &self,
        provider_payment_id: &str,
        next: PaymentState,
    ) -> Result<(), PaymentProviderError> {
        let mut ledger = self
            .ledger
            .lock()
            .map_err(|_| Self::poisoned(provider_payment_id))?;
        if !ledger.states.contains_key(provider_payment_id) {
            return Err(PaymentProviderError::PaymentNotFound {
                provider_payment_id: provider_payment_id.into(),
            });
        }
        ledger.states.insert(provider_payment_id.into(), next);
        Ok(())
    }

    fn poisoned(provider_payment_id: &str) -> PaymentProviderError {
        PaymentProviderError::Indeterminate {
            provider: PROVIDER_CODE.into(),
            provider_payment_id: Some(provider_payment_id.into()),
            message: "simulated ledger lock poisoned".into(),
        }
    }
}

#[async_trait]
impl PaymentProvider for SimulatedTerminalProvider {
    fn provider_code(&self) -> &'static str {
        PROVIDER_CODE
    }

    async fn authorize(
        &self,
        request: AuthorizeRequest,
    ) -> Result<PaymentAuthorization, PaymentProviderError> {
        validate_amount(request.amount_cents)?;

        if let Ok(ledger) = self.ledger.lock() {
            if let Some(existing) = ledger.by_idempotency_key.get(&request.idempotency_key) {
                return Ok(existing.clone());
            }
        }

        let suffix = request.amount_cents % 100;
        if suffix == SIMULATED_TIMEOUT_SUFFIX {
            return Err(PaymentProviderError::Timeout {
                provider: PROVIDER_CODE.into(),
            });
        }
        if suffix == SIMULATED_INDETERMINATE_SUFFIX {
            return Err(PaymentProviderError::Indeterminate {
                provider: PROVIDER_CODE.into(),
                provider_payment_id: Some(format!("sim_{}", request.idempotency_key)),
                message: "simulated loss of contact after card was presented".into(),
            });
        }

        let provider_payment_id = format!("sim_{}", request.idempotency_key);
        let (approved_cents, outcome) = if suffix == SIMULATED_DECLINE_SUFFIX {
            (
                0,
                AuthorizationOutcome::Declined {
                    code: "card_declined".into(),
                    message: "The card was declined.".into(),
                },
            )
        } else if suffix == SIMULATED_PARTIAL_SUFFIX {
            (
                request.amount_cents / 2,
                AuthorizationOutcome::PartiallyApproved {
                    requested_cents: request.amount_cents,
                },
            )
        } else {
            (request.amount_cents, AuthorizationOutcome::Approved)
        };

        let authorization = PaymentAuthorization {
            provider: PROVIDER_CODE.into(),
            provider_payment_id: provider_payment_id.clone(),
            approved_cents,
            tip_amount_cents: request.tip_amount_cents,
            outcome,
            receipt: Some(Self::receipt(&provider_payment_id)),
        };

        if let Ok(mut ledger) = self.ledger.lock() {
            ledger
                .by_idempotency_key
                .insert(request.idempotency_key, authorization.clone());
            if authorization.is_approved() {
                ledger
                    .states
                    .insert(provider_payment_id, PaymentState::Authorized);
            }
        }

        Ok(authorization)
    }

    async fn capture(
        &self,
        request: CaptureRequest,
    ) -> Result<PaymentCapture, PaymentProviderError> {
        validate_amount(request.amount_cents)?;
        self.transition(&request.provider_payment_id, PaymentState::Captured)?;
        Ok(PaymentCapture {
            provider: PROVIDER_CODE.into(),
            provider_payment_id: request.provider_payment_id.clone(),
            captured_cents: request.amount_cents,
            receipt: Self::receipt(&request.provider_payment_id),
        })
    }

    async fn void(&self, request: VoidRequest) -> Result<(), PaymentProviderError> {
        self.transition(&request.provider_payment_id, PaymentState::Reversed)
    }

    async fn refund(&self, request: RefundRequest) -> Result<PaymentRefund, PaymentProviderError> {
        validate_amount(request.amount_cents)?;
        self.transition(&request.provider_payment_id, PaymentState::Reversed)?;
        Ok(PaymentRefund {
            provider: PROVIDER_CODE.into(),
            provider_refund_id: format!("sim_refund_{}", request.idempotency_key),
            refunded_cents: request.amount_cents,
            receipt: Self::receipt(&request.provider_payment_id),
        })
    }
}
