//! Behavioural coverage for card payments wired into the POS tender path.
//!
//! The property that matters is not "a happy path works" but "the hub never ends up
//! holding money for a sale it did not complete". These tests drive declines, partial
//! approvals, indeterminate provider answers, and a finalize that fails *after* the card
//! was captured, then assert the reversal sweeper gives the money back.

use std::sync::Arc;

use apex_edge_adapters_fiscal::DeTseFiscalProvider;
use apex_edge_adapters_payment::{
    SimulatedTerminalProvider, SIMULATED_DECLINE_SUFFIX, SIMULATED_INDETERMINATE_SUFFIX,
    SIMULATED_PARTIAL_SUFFIX,
};
use apex_edge_api::{
    handle_pos_command, run_payment_reversal_sweep, AppState, FiscalSettings, PaymentSettings,
    ReversalSweepPolicy,
};
use apex_edge_contracts::{
    AddLineItemPayload, AddPaymentPayload, CartState, ContractVersion, CreateCartPayload,
    FinalizeOrderPayload, PosCommand, PosRequestEnvelope, SetTenderingPayload,
};
use apex_edge_storage::{
    insert_catalog_item, insert_price_book_entry, list_payment_intents_for_cart, run_migrations,
    PaymentIntentState,
};
use axum::extract::State;
use axum::Json;
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

const PROVIDER: &str = "simulated_terminal";

struct Fixture {
    state: AppState,
    store_id: Uuid,
    register_id: Uuid,
    terminal: Arc<SimulatedTerminalProvider>,
}

async fn fixture(fiscal: FiscalSettings) -> Fixture {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let terminal = Arc::new(SimulatedTerminalProvider::new());
    let payments = PaymentSettings::new("EUR").with_provider(terminal.clone());
    let store_id = Uuid::nil();

    Fixture {
        state: AppState {
            fiscal,
            payments,
            ..AppState::new(pool, store_id)
        },
        store_id,
        register_id: Uuid::new_v4(),
        terminal,
    }
}

impl Fixture {
    /// Create a cart holding a single line priced at `unit_price_cents`, moved to tendering.
    async fn tendering_cart(&self, unit_price_cents: u64) -> Uuid {
        let item_id = Uuid::new_v4();
        insert_catalog_item(
            &self.state.pool,
            item_id,
            self.store_id,
            "PAY-001",
            "Payment Test Item",
            Uuid::new_v4(),
            Uuid::new_v4(),
        )
        .await
        .expect("insert_catalog_item");
        insert_price_book_entry(
            &self.state.pool,
            self.store_id,
            item_id,
            None,
            unit_price_cents,
            "EUR",
        )
        .await
        .expect("insert_price_book_entry");

        let created = self
            .send(PosCommand::CreateCart(CreateCartPayload { cart_id: None }))
            .await;
        let cart: CartState =
            serde_json::from_value(created.payload.expect("cart payload")).expect("cart state");

        self.send(PosCommand::AddLineItem(AddLineItemPayload {
            cart_id: cart.cart_id,
            item_id,
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }))
        .await;
        self.send(PosCommand::SetTendering(SetTenderingPayload {
            cart_id: cart.cart_id,
        }))
        .await;

        cart.cart_id
    }

    async fn send(
        &self,
        payload: PosCommand,
    ) -> apex_edge_contracts::PosResponseEnvelope<serde_json::Value> {
        handle_pos_command(
            State(self.state.clone()),
            None,
            Json(PosRequestEnvelope {
                version: ContractVersion::V1_0_0,
                idempotency_key: Uuid::new_v4(),
                store_id: self.store_id,
                register_id: self.register_id,
                payload,
            }),
        )
        .await
        .0
    }

    async fn pay_by_card(
        &self,
        cart_id: Uuid,
        amount_cents: u64,
    ) -> apex_edge_contracts::PosResponseEnvelope<serde_json::Value> {
        self.send(PosCommand::AddPayment(AddPaymentPayload {
            cart_id,
            tender_id: Uuid::new_v4(),
            amount_cents,
            tip_amount_cents: 0,
            external_reference: None,
            provider: Some(PROVIDER.into()),
            provider_payment_id: None,
            entry_method: None,
        }))
        .await
    }

    async fn intents(&self, cart_id: Uuid) -> Vec<apex_edge_storage::PaymentIntentRecord> {
        list_payment_intents_for_cart(&self.state.pool, self.store_id, cart_id)
            .await
            .expect("list intents")
    }
}

#[tokio::test]
async fn a_card_payment_authorizes_captures_and_settles_on_finalize() {
    let f = fixture(FiscalSettings::default()).await;
    let cart_id = f.tendering_cart(2_500).await;

    let paid = f.pay_by_card(cart_id, 2_500).await;
    assert!(paid.success, "errors: {:?}", paid.errors);

    let intents = f.intents(cart_id).await;
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].state, PaymentIntentState::Captured);
    assert_eq!(intents[0].approved_cents, 2_500);
    assert!(intents[0].provider_payment_id.is_some());

    let finalized = f
        .send(PosCommand::FinalizeOrder(FinalizeOrderPayload { cart_id }))
        .await;
    assert!(finalized.success, "errors: {:?}", finalized.errors);

    let intents = f.intents(cart_id).await;
    assert_eq!(intents[0].state, PaymentIntentState::Settled);
    assert!(intents[0].order_id.is_some());
}

#[tokio::test]
async fn a_declined_card_leaves_the_cart_unpaid_and_records_no_tender() {
    let f = fixture(FiscalSettings::default()).await;
    let amount = 2_500 + SIMULATED_DECLINE_SUFFIX;
    let cart_id = f.tendering_cart(amount).await;

    let declined = f.pay_by_card(cart_id, amount).await;

    assert!(
        !declined.success,
        "a declined card must not record a tender"
    );
    assert_eq!(declined.errors.len(), 1);
    assert_eq!(declined.errors[0].code, "PAYMENT_DECLINED");

    let intents = f.intents(cart_id).await;
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].state, PaymentIntentState::Declined);

    let finalized = f
        .send(PosCommand::FinalizeOrder(FinalizeOrderPayload { cart_id }))
        .await;
    assert!(
        !finalized.success,
        "an unpaid cart must not be finalizable after a decline"
    );
}

#[tokio::test]
async fn a_partial_approval_records_only_what_the_card_approved() {
    let f = fixture(FiscalSettings::default()).await;
    let amount = 4_000 + SIMULATED_PARTIAL_SUFFIX;
    let cart_id = f.tendering_cart(amount).await;

    let paid = f.pay_by_card(cart_id, amount).await;
    assert!(paid.success, "errors: {:?}", paid.errors);

    let cart: CartState = serde_json::from_value(paid.payload.expect("payload")).expect("cart");
    assert_eq!(
        cart.tendered_cents,
        amount / 2,
        "the cart must reflect the approved amount, not the requested one"
    );
    assert_eq!(
        cart.state,
        apex_edge_contracts::CartStateKind::Tendering,
        "a partially paid cart still owes money"
    );

    let intents = f.intents(cart_id).await;
    assert_eq!(intents[0].approved_cents, amount / 2);
}

#[tokio::test]
async fn an_unconfigured_provider_is_refused_rather_than_recorded_as_paid() {
    let f = fixture(FiscalSettings::default()).await;
    let cart_id = f.tendering_cart(2_500).await;

    let response = f
        .send(PosCommand::AddPayment(AddPaymentPayload {
            cart_id,
            tender_id: Uuid::new_v4(),
            amount_cents: 2_500,
            tip_amount_cents: 0,
            external_reference: None,
            provider: Some("adyen_terminal".into()),
            provider_payment_id: None,
            entry_method: None,
        }))
        .await;

    assert!(!response.success);
    assert_eq!(response.errors[0].code, "PAYMENT_PROVIDER_UNKNOWN");
    assert!(f.intents(cart_id).await.is_empty());
}

#[tokio::test]
async fn a_tender_the_pos_settled_itself_still_records_without_a_provider() {
    // Cash counted into the drawer, gift card balances, and externally captured cards
    // all arrive with no provider. That path must keep working untouched.
    let f = fixture(FiscalSettings::default()).await;
    let cart_id = f.tendering_cart(1_000).await;

    let paid = f
        .send(PosCommand::AddPayment(AddPaymentPayload {
            cart_id,
            tender_id: Uuid::new_v4(),
            amount_cents: 1_000,
            tip_amount_cents: 0,
            external_reference: Some("cash".into()),
            provider: None,
            provider_payment_id: None,
            entry_method: None,
        }))
        .await;

    assert!(paid.success, "errors: {:?}", paid.errors);
    assert!(
        f.intents(cart_id).await.is_empty(),
        "a manual tender contacts no provider, so it opens no intent"
    );
}

#[tokio::test]
async fn an_indeterminate_authorization_is_queued_for_reversal_not_treated_as_unpaid() {
    let f = fixture(FiscalSettings::default()).await;
    let amount = 2_500 + SIMULATED_INDETERMINATE_SUFFIX;
    let cart_id = f.tendering_cart(amount).await;

    let response = f.pay_by_card(cart_id, amount).await;

    assert!(!response.success);
    assert_eq!(response.errors[0].code, "PAYMENT_INDETERMINATE");

    let intents = f.intents(cart_id).await;
    assert_eq!(intents.len(), 1);
    assert_eq!(
        intents[0].state,
        PaymentIntentState::ReversalPending,
        "an unknown provider answer must be resolved by the sweeper, not assumed safe"
    );
}

#[tokio::test]
async fn a_finalize_that_fails_after_capture_flags_the_payment_and_the_sweeper_reverses_it() {
    // An unconfigured regulated fiscal provider fails finalize closed *after* the card has
    // already been captured. This is the exact window that would otherwise take a
    // customer's money and give them no sale.
    let f = fixture(FiscalSettings {
        provider: Arc::new(DeTseFiscalProvider::new(false)),
        currency: "EUR".into(),
    })
    .await;
    let cart_id = f.tendering_cart(3_300).await;

    let paid = f.pay_by_card(cart_id, 3_300).await;
    assert!(paid.success, "errors: {:?}", paid.errors);
    let captured_payment_id = f.intents(cart_id).await[0]
        .provider_payment_id
        .clone()
        .expect("captured payment id");

    let finalized = f
        .send(PosCommand::FinalizeOrder(FinalizeOrderPayload { cart_id }))
        .await;
    assert!(!finalized.success, "fiscal signing must fail closed");

    let intents = f.intents(cart_id).await;
    assert_eq!(
        intents[0].state,
        PaymentIntentState::ReversalPending,
        "money taken for a sale that failed must be flagged"
    );
    assert!(
        !f.terminal.is_reversed(&captured_payment_id),
        "not reversed until the sweeper runs"
    );

    let reversed = run_payment_reversal_sweep(
        &f.state.pool,
        &f.state.payments,
        ReversalSweepPolicy::default(),
    )
    .await
    .expect("sweep");

    assert_eq!(reversed, 1);
    assert!(
        f.terminal.is_reversed(&captured_payment_id),
        "the customer must get their money back"
    );
    assert_eq!(
        f.intents(cart_id).await[0].state,
        PaymentIntentState::Reversed
    );
}

#[tokio::test]
async fn a_capture_orphaned_by_a_crash_is_found_and_reversed_without_any_failure_handler() {
    // Simulates the process dying immediately after the card was captured: no error path
    // ran, so nothing flagged the payment. The only thing standing between the customer
    // and lost money is the sweeper noticing an old capture with no order.
    let f = fixture(FiscalSettings::default()).await;
    let cart_id = f.tendering_cart(2_500).await;
    f.pay_by_card(cart_id, 2_500).await;

    let payment_id = f.intents(cart_id).await[0]
        .provider_payment_id
        .clone()
        .expect("payment id");
    assert_eq!(
        f.intents(cart_id).await[0].state,
        PaymentIntentState::Captured
    );

    // Nothing else happens: no finalize, no void, no error handler.
    let policy = ReversalSweepPolicy {
        batch: 10,
        stale_capture_after: std::time::Duration::from_secs(1),
    };
    assert_eq!(
        run_payment_reversal_sweep(&f.state.pool, &f.state.payments, policy)
            .await
            .expect("sweep"),
        0,
        "a checkout still in progress must be left alone"
    );

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let reversed = run_payment_reversal_sweep(&f.state.pool, &f.state.payments, policy)
        .await
        .expect("sweep");

    assert_eq!(reversed, 1, "an orphaned capture must be given back");
    assert!(f.terminal.is_reversed(&payment_id));
    assert_eq!(
        f.intents(cart_id).await[0].state,
        PaymentIntentState::Reversed
    );
}

#[tokio::test]
async fn the_sweeper_never_reverses_a_payment_that_earned_a_real_order() {
    let f = fixture(FiscalSettings::default()).await;
    let cart_id = f.tendering_cart(2_500).await;
    f.pay_by_card(cart_id, 2_500).await;
    let payment_id = f.intents(cart_id).await[0]
        .provider_payment_id
        .clone()
        .expect("payment id");

    let finalized = f
        .send(PosCommand::FinalizeOrder(FinalizeOrderPayload { cart_id }))
        .await;
    assert!(finalized.success, "errors: {:?}", finalized.errors);

    let reversed = run_payment_reversal_sweep(
        &f.state.pool,
        &f.state.payments,
        ReversalSweepPolicy::default(),
    )
    .await
    .expect("sweep");

    assert_eq!(reversed, 0);
    assert!(!f.terminal.is_reversed(&payment_id));
}

#[tokio::test]
async fn retrying_the_same_payment_command_does_not_charge_twice() {
    let f = fixture(FiscalSettings::default()).await;
    let cart_id = f.tendering_cart(2_500).await;
    let tender_id = Uuid::new_v4();
    let idempotency_key = Uuid::new_v4();

    let command = PosCommand::AddPayment(AddPaymentPayload {
        cart_id,
        tender_id,
        amount_cents: 2_500,
        tip_amount_cents: 0,
        external_reference: None,
        provider: Some(PROVIDER.into()),
        provider_payment_id: None,
        entry_method: None,
    });

    for _ in 0..2 {
        let response = handle_pos_command(
            State(f.state.clone()),
            None,
            Json(PosRequestEnvelope {
                version: ContractVersion::V1_0_0,
                idempotency_key,
                store_id: f.store_id,
                register_id: f.register_id,
                payload: command.clone(),
            }),
        )
        .await;
        assert!(response.0.success, "errors: {:?}", response.0.errors);
    }

    assert_eq!(
        f.intents(cart_id).await.len(),
        1,
        "a retried tender must not open a second intent"
    );
    assert_eq!(f.terminal.authorization_count(), 1);
}
