//! Stripe Terminal server-driven integration, exercised against a local stand-in for
//! the Stripe API.
//!
//! The real integration is pure REST (no Terminal SDK), so the whole flow can be
//! verified without a Stripe account or a physical reader: these tests assert the
//! requests ApexEdge sends and how it interprets reader/PaymentIntent state.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use apex_edge_adapters_payment::{
    AuthorizeRequest, CaptureRequest, PaymentProvider, PaymentProviderError, RefundRequest,
    StripeTerminalConfig, StripeTerminalProvider, VoidRequest,
};
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use uuid::Uuid;

#[derive(Clone, Default)]
struct FakeStripe {
    /// Every path the provider called, in order.
    calls: Arc<Mutex<Vec<String>>>,
    /// Idempotency-Key headers seen, to prove they are sent.
    idempotency_keys: Arc<Mutex<Vec<String>>>,
    /// When set, the reader action reports this failure code instead of succeeding.
    failure_code: Arc<Mutex<Option<String>>>,
}

impl FakeStripe {
    fn record(&self, path: &str) {
        self.calls
            .lock()
            .expect("calls lock")
            .push(path.to_string());
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls lock").clone()
    }
}

async fn create_payment_intent(
    State(state): State<FakeStripe>,
    body: String,
) -> Json<serde_json::Value> {
    state.record("create_payment_intent");
    assert!(
        body.contains("capture_method=manual"),
        "terminal payments must authorize first and capture after the sale is safe, got: {body}"
    );
    assert!(
        body.contains("payment_method_types%5B%5D=card_present")
            || body.contains("payment_method_types[]=card_present"),
        "card_present is required for Terminal, got: {body}"
    );
    Json(serde_json::json!({
        "id": "pi_test_123",
        "object": "payment_intent",
        "amount": 2_500,
        "currency": "eur",
        "status": "requires_payment_method",
    }))
}

async fn process_payment_intent(
    State(state): State<FakeStripe>,
    Path(reader_id): Path<String>,
    headers: axum::http::HeaderMap,
    _body: String,
) -> Json<serde_json::Value> {
    state.record("process_payment_intent");
    assert_eq!(reader_id, "tmr_simulated");
    if let Some(key) = headers.get("idempotency-key").and_then(|v| v.to_str().ok()) {
        state
            .idempotency_keys
            .lock()
            .expect("keys lock")
            .push(key.to_string());
    }
    Json(serde_json::json!({
        "id": reader_id,
        "object": "terminal.reader",
        "action": { "status": "in_progress", "type": "process_payment_intent" },
    }))
}

async fn get_reader(
    State(state): State<FakeStripe>,
    Path(reader_id): Path<String>,
) -> Json<serde_json::Value> {
    state.record("get_reader");
    let failure = state.failure_code.lock().expect("failure lock").clone();
    match failure {
        Some(code) => Json(serde_json::json!({
            "id": reader_id,
            "object": "terminal.reader",
            "action": {
                "status": "failed",
                "type": "process_payment_intent",
                "failure_code": code,
                "failure_message": "The card was declined.",
            },
        })),
        None => Json(serde_json::json!({
            "id": reader_id,
            "object": "terminal.reader",
            "action": {
                "status": "succeeded",
                "type": "process_payment_intent",
                "process_payment_intent": { "payment_intent": "pi_test_123" },
            },
        })),
    }
}

async fn get_payment_intent(
    State(state): State<FakeStripe>,
    Path(intent_id): Path<String>,
) -> Json<serde_json::Value> {
    state.record("get_payment_intent");
    Json(serde_json::json!({
        "id": intent_id,
        "object": "payment_intent",
        "amount": 2_500,
        "amount_received": 2_500,
        "status": "requires_capture",
        "latest_charge": {
            "payment_method_details": {
                "card_present": {
                    "last4": "4242",
                    "network": "visa",
                    "read_method": "contactless_emv",
                    "receipt": { "application_preferred_name": "Visa", "dedicated_file_name": "A0000000031010" }
                }
            }
        }
    }))
}

async fn capture_payment_intent(
    State(state): State<FakeStripe>,
    Path(intent_id): Path<String>,
) -> Json<serde_json::Value> {
    state.record("capture_payment_intent");
    Json(serde_json::json!({
        "id": intent_id,
        "object": "payment_intent",
        "amount": 2_500,
        "amount_received": 2_500,
        "status": "succeeded",
    }))
}

async fn cancel_payment_intent(
    State(state): State<FakeStripe>,
    Path(intent_id): Path<String>,
) -> Json<serde_json::Value> {
    state.record("cancel_payment_intent");
    Json(serde_json::json!({
        "id": intent_id,
        "object": "payment_intent",
        "status": "canceled",
    }))
}

async fn create_refund(State(state): State<FakeStripe>, _body: String) -> Json<serde_json::Value> {
    state.record("create_refund");
    Json(serde_json::json!({
        "id": "re_test_456",
        "object": "refund",
        "amount": 1_000,
        "status": "succeeded",
    }))
}

async fn spawn_fake_stripe(state: FakeStripe) -> SocketAddr {
    let app = Router::new()
        .route("/v1/payment_intents", post(create_payment_intent))
        .route("/v1/payment_intents/:id", get(get_payment_intent))
        .route(
            "/v1/payment_intents/:id/capture",
            post(capture_payment_intent),
        )
        .route(
            "/v1/payment_intents/:id/cancel",
            post(cancel_payment_intent),
        )
        .route(
            "/v1/terminal/readers/:id/process_payment_intent",
            post(process_payment_intent),
        )
        .route("/v1/terminal/readers/:id", get(get_reader))
        .route("/v1/refunds", post(create_refund))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake stripe");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

fn provider(addr: SocketAddr) -> StripeTerminalProvider {
    StripeTerminalProvider::new(StripeTerminalConfig {
        secret_key: "sk_test_fake".into(),
        reader_id: "tmr_simulated".into(),
        api_base_url: format!("http://{addr}"),
        poll_interval: std::time::Duration::from_millis(1),
        poll_timeout: std::time::Duration::from_secs(5),
    })
    .expect("provider builds")
}

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
async fn authorize_drives_the_reader_and_reads_card_metadata() {
    let state = FakeStripe::default();
    let addr = spawn_fake_stripe(state.clone()).await;
    let provider = provider(addr);

    let auth = provider
        .authorize(authorize_request(2_500))
        .await
        .expect("authorize succeeds");

    assert!(auth.is_approved());
    assert_eq!(auth.provider, "stripe_terminal");
    assert_eq!(auth.provider_payment_id, "pi_test_123");
    assert_eq!(auth.approved_cents, 2_500);

    let receipt = auth.receipt.expect("receipt metadata");
    assert_eq!(receipt.last4.as_deref(), Some("4242"));
    assert_eq!(receipt.aid.as_deref(), Some("A0000000031010"));

    let calls = state.calls();
    assert_eq!(
        calls.first().map(String::as_str),
        Some("create_payment_intent")
    );
    assert!(calls.contains(&"process_payment_intent".to_string()));
    assert!(calls.contains(&"get_reader".to_string()));
}

#[tokio::test]
async fn authorize_sends_an_idempotency_key() {
    let state = FakeStripe::default();
    let addr = spawn_fake_stripe(state.clone()).await;
    let provider = provider(addr);

    provider
        .authorize(authorize_request(2_500))
        .await
        .expect("authorize succeeds");

    let keys = state.idempotency_keys.lock().expect("keys lock").clone();
    assert!(
        !keys.is_empty(),
        "a retried command must not start a second reader payment"
    );
}

#[tokio::test]
async fn a_declined_reader_action_becomes_a_decline_outcome() {
    let state = FakeStripe::default();
    *state.failure_code.lock().expect("failure lock") = Some("card_declined".into());
    let addr = spawn_fake_stripe(state.clone()).await;
    let provider = provider(addr);

    let auth = provider
        .authorize(authorize_request(2_500))
        .await
        .expect("a decline is not a transport error");

    assert!(!auth.is_approved());
    let (code, _) = auth.declined_reason().expect("decline reason");
    assert_eq!(code, "card_declined");
}

#[tokio::test]
async fn capture_void_and_refund_hit_the_expected_endpoints() {
    let state = FakeStripe::default();
    let addr = spawn_fake_stripe(state.clone()).await;
    let provider = provider(addr);

    let capture = provider
        .capture(CaptureRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: "pi_test_123".into(),
            amount_cents: 2_500,
        })
        .await
        .expect("capture");
    assert_eq!(capture.captured_cents, 2_500);

    provider
        .void(VoidRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: "pi_test_123".into(),
            reason: None,
        })
        .await
        .expect("void");

    let refund = provider
        .refund(RefundRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: "pi_test_123".into(),
            amount_cents: 1_000,
            reason: None,
        })
        .await
        .expect("refund");
    assert_eq!(refund.provider_refund_id, "re_test_456");
    assert_eq!(refund.refunded_cents, 1_000);

    let calls = state.calls();
    assert!(calls.contains(&"capture_payment_intent".to_string()));
    assert!(calls.contains(&"cancel_payment_intent".to_string()));
    assert!(calls.contains(&"create_refund".to_string()));
}

#[tokio::test]
async fn an_unreachable_stripe_is_indeterminate_not_a_silent_success() {
    // Port 1 is reserved and refuses connections, standing in for a network partition
    // after the reader may already have taken the card.
    let provider = StripeTerminalProvider::new(StripeTerminalConfig {
        secret_key: "sk_test_fake".into(),
        reader_id: "tmr_simulated".into(),
        api_base_url: "http://127.0.0.1:1".into(),
        poll_interval: std::time::Duration::from_millis(1),
        poll_timeout: std::time::Duration::from_millis(50),
    })
    .expect("provider builds");

    let err = provider
        .capture(CaptureRequest {
            idempotency_key: Uuid::new_v4(),
            provider_payment_id: "pi_test_123".into(),
            amount_cents: 2_500,
        })
        .await
        .expect_err("unreachable stripe");

    assert!(
        err.requires_reversal_check(),
        "a failed capture may still have taken the money: {err}"
    );
    assert!(matches!(
        err,
        PaymentProviderError::Indeterminate { .. } | PaymentProviderError::Transport { .. }
    ));
}

#[tokio::test]
async fn a_missing_secret_key_fails_closed() {
    let err = StripeTerminalProvider::new(StripeTerminalConfig {
        secret_key: String::new(),
        reader_id: "tmr_simulated".into(),
        api_base_url: "https://api.stripe.com".into(),
        poll_interval: std::time::Duration::from_millis(1),
        poll_timeout: std::time::Duration::from_secs(1),
    })
    .expect_err("empty secret key must not build a provider");

    assert!(matches!(err, PaymentProviderError::NotConfigured { .. }));
}
