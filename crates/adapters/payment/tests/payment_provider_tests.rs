//! Behavioural tests for the payment provider contract.
//!
//! These cover the outcomes a store hub must survive: declines, partial approvals,
//! timeouts, indeterminate results where money may have moved, idempotent retries,
//! and reversal of a captured payment.

use apex_edge_adapters_payment::{
    AuthorizationOutcome, AuthorizeRequest, CaptureRequest, CashPaymentProvider, PaymentProvider,
    PaymentProviderError, RefundRequest, SimulatedTerminalProvider, VoidRequest,
    SIMULATED_DECLINE_SUFFIX, SIMULATED_INDETERMINATE_SUFFIX, SIMULATED_PARTIAL_SUFFIX,
    SIMULATED_TIMEOUT_SUFFIX,
};
use apex_edge_contracts::PaymentEntryMethod;
use uuid::Uuid;

fn authorize_request(amount_cents: u64) -> AuthorizeRequest {
    AuthorizeRequest {
        idempotency_key: Uuid::new_v4(),
        cart_id: Uuid::new_v4(),
        store_id: Uuid::new_v4(),
        register_id: Uuid::new_v4(),
        amount_cents,
        tip_amount_cents: 0,
        currency: "EUR".into(),
    }
}

#[tokio::test]
async fn cash_authorize_and_capture_records_cash_entry_method() {
    let provider = CashPaymentProvider;
    let auth = provider
        .authorize(authorize_request(1_000))
        .await
        .expect("cash authorizes");

    assert_eq!(auth.provider, "cash");
    assert!(auth.is_approved());
    assert_eq!(auth.approved_cents, 1_000);

    let capture = provider
        .capture(CaptureRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: auth.provider_payment_id.clone(),
            amount_cents: auth.approved_cents,
        })
        .await
        .expect("cash captures");

    assert_eq!(capture.captured_cents, 1_000);
    assert_eq!(capture.receipt.entry_method, Some(PaymentEntryMethod::Cash));
}

#[tokio::test]
async fn zero_amount_is_rejected_before_reaching_a_provider() {
    let provider = CashPaymentProvider;
    let err = provider
        .authorize(authorize_request(0))
        .await
        .expect_err("zero amount must be rejected");

    assert_eq!(err, PaymentProviderError::InvalidAmount);
}

#[tokio::test]
async fn simulated_terminal_approves_and_reports_contactless_entry() {
    let provider = SimulatedTerminalProvider::new();
    let auth = provider
        .authorize(authorize_request(2_500))
        .await
        .expect("simulated terminal authorizes");

    assert!(auth.is_approved());
    assert_eq!(auth.outcome, AuthorizationOutcome::Approved);

    let capture = provider
        .capture(CaptureRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: auth.provider_payment_id.clone(),
            amount_cents: auth.approved_cents,
        })
        .await
        .expect("simulated terminal captures");

    assert_eq!(
        capture.receipt.entry_method,
        Some(PaymentEntryMethod::Contactless)
    );
    assert_eq!(capture.receipt.last4.as_deref(), Some("4242"));
}

#[tokio::test]
async fn simulated_terminal_declines_are_outcomes_not_errors() {
    let provider = SimulatedTerminalProvider::new();
    let auth = provider
        .authorize(authorize_request(1_000 + SIMULATED_DECLINE_SUFFIX))
        .await
        .expect("a decline is a business outcome, not a transport error");

    assert!(!auth.is_approved());
    assert_eq!(auth.approved_cents, 0);
    let (code, _) = auth.declined_reason().expect("decline carries a reason");
    assert_eq!(code, "card_declined");
}

#[tokio::test]
async fn simulated_terminal_partial_approval_reports_requested_amount() {
    let provider = SimulatedTerminalProvider::new();
    let requested = 4_000 + SIMULATED_PARTIAL_SUFFIX;
    let auth = provider
        .authorize(authorize_request(requested))
        .await
        .expect("partial approval authorizes");

    assert_eq!(
        auth.outcome,
        AuthorizationOutcome::PartiallyApproved {
            requested_cents: requested
        }
    );
    assert!(auth.approved_cents > 0);
    assert!(auth.approved_cents < requested);
    assert!(auth.is_approved());
}

#[tokio::test]
async fn simulated_terminal_timeout_is_reversal_safe_error() {
    let provider = SimulatedTerminalProvider::new();
    let err = provider
        .authorize(authorize_request(1_000 + SIMULATED_TIMEOUT_SUFFIX))
        .await
        .expect_err("timeout must surface as an error");

    assert!(matches!(err, PaymentProviderError::Timeout { .. }));
    assert!(
        !err.requires_reversal_check(),
        "an authorize timeout never captured funds"
    );
}

#[tokio::test]
async fn simulated_terminal_indeterminate_result_demands_a_reversal_check() {
    let provider = SimulatedTerminalProvider::new();
    let err = provider
        .authorize(authorize_request(1_000 + SIMULATED_INDETERMINATE_SUFFIX))
        .await
        .expect_err("indeterminate must surface as an error");

    assert!(
        err.requires_reversal_check(),
        "money may have moved; the sweeper must resolve it"
    );
}

#[tokio::test]
async fn repeating_an_authorization_with_the_same_key_does_not_double_charge() {
    let provider = SimulatedTerminalProvider::new();
    let request = authorize_request(3_000);

    let first = provider
        .authorize(request.clone())
        .await
        .expect("first authorize");
    let second = provider
        .authorize(request)
        .await
        .expect("retry with same idempotency key");

    assert_eq!(first.provider_payment_id, second.provider_payment_id);
    assert_eq!(provider.authorization_count(), 1);
}

#[tokio::test]
async fn voiding_a_captured_payment_reverses_it_once() {
    let provider = SimulatedTerminalProvider::new();
    let auth = provider
        .authorize(authorize_request(1_500))
        .await
        .expect("authorize");
    provider
        .capture(CaptureRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: auth.provider_payment_id.clone(),
            amount_cents: auth.approved_cents,
        })
        .await
        .expect("capture");

    provider
        .void(VoidRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: auth.provider_payment_id.clone(),
            reason: Some("finalize failed".into()),
        })
        .await
        .expect("void succeeds");

    assert!(provider.is_reversed(&auth.provider_payment_id));
}

#[tokio::test]
async fn refunding_returns_a_distinct_refund_id() {
    let provider = SimulatedTerminalProvider::new();
    let auth = provider
        .authorize(authorize_request(5_000))
        .await
        .expect("authorize");
    provider
        .capture(CaptureRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: auth.provider_payment_id.clone(),
            amount_cents: auth.approved_cents,
        })
        .await
        .expect("capture");

    let refund = provider
        .refund(RefundRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: auth.provider_payment_id.clone(),
            amount_cents: 2_000,
            reason: Some("customer return".into()),
        })
        .await
        .expect("refund");

    assert_eq!(refund.refunded_cents, 2_000);
    assert_ne!(refund.provider_refund_id, auth.provider_payment_id);
}

#[tokio::test]
async fn voiding_an_unknown_payment_reports_not_found() {
    let provider = SimulatedTerminalProvider::new();
    let err = provider
        .void(VoidRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: "does_not_exist".into(),
            reason: None,
        })
        .await
        .expect_err("unknown payment");

    assert!(matches!(err, PaymentProviderError::PaymentNotFound { .. }));
}
