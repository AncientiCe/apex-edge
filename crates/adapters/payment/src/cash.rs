//! Cash tender. Authorization and capture are local bookkeeping: the drawer is the
//! source of truth, so there is nothing to contact and nothing that can time out.

use apex_edge_contracts::{PaymentEntryMethod, PaymentProviderReceipt};
use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    validate_amount, AuthorizationOutcome, AuthorizeRequest, CaptureRequest, PaymentAuthorization,
    PaymentCapture, PaymentProvider, PaymentProviderError, PaymentRefund, RefundRequest,
    VoidRequest,
};

#[derive(Debug, Clone, Default)]
pub struct CashPaymentProvider;

impl CashPaymentProvider {
    fn receipt(provider_payment_id: &str) -> PaymentProviderReceipt {
        PaymentProviderReceipt {
            provider: "cash".into(),
            provider_payment_id: provider_payment_id.into(),
            entry_method: Some(PaymentEntryMethod::Cash),
            last4: None,
            aid: None,
            authorization_code: None,
        }
    }
}

#[async_trait]
impl PaymentProvider for CashPaymentProvider {
    fn provider_code(&self) -> &'static str {
        "cash"
    }

    async fn authorize(
        &self,
        request: AuthorizeRequest,
    ) -> Result<PaymentAuthorization, PaymentProviderError> {
        validate_amount(request.amount_cents)?;
        // Deriving the id from the idempotency key makes a retried AddPayment resolve to
        // the same cash tender rather than a second one.
        let provider_payment_id = format!("cash_{}", request.idempotency_key);
        Ok(PaymentAuthorization {
            provider: self.provider_code().into(),
            provider_payment_id: provider_payment_id.clone(),
            approved_cents: request.amount_cents,
            tip_amount_cents: request.tip_amount_cents,
            outcome: AuthorizationOutcome::Approved,
            receipt: Some(Self::receipt(&provider_payment_id)),
        })
    }

    async fn capture(
        &self,
        request: CaptureRequest,
    ) -> Result<PaymentCapture, PaymentProviderError> {
        validate_amount(request.amount_cents)?;
        Ok(PaymentCapture {
            provider: self.provider_code().into(),
            provider_payment_id: request.provider_payment_id.clone(),
            captured_cents: request.amount_cents,
            receipt: Self::receipt(&request.provider_payment_id),
        })
    }

    async fn void(&self, _request: VoidRequest) -> Result<(), PaymentProviderError> {
        Ok(())
    }

    async fn refund(&self, request: RefundRequest) -> Result<PaymentRefund, PaymentProviderError> {
        validate_amount(request.amount_cents)?;
        Ok(PaymentRefund {
            provider: self.provider_code().into(),
            provider_refund_id: format!("cash_refund_{}", Uuid::new_v4()),
            refunded_cents: request.amount_cents,
            receipt: Self::receipt(&request.provider_payment_id),
        })
    }
}
