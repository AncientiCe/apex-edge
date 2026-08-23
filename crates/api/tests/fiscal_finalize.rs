//! Behavioural coverage for fiscal receipt signing wired into order finalize.
//!
//! `PosCommand::FinalizeOrder` calls the configured `FiscalProvider` before any ledger,
//! outbox, or stock mutation happens: a successful signature is persisted on the order
//! row, and a failed signature (e.g. an unconfigured regulated provider) fails the whole
//! finalize closed, leaving no order behind.

use apex_edge_adapters_fiscal::{
    DeTseFiscalProvider, FiscalError, FiscalProvider, FiscalSignature, FiscalTransaction,
    OfflinePolicy,
};
use apex_edge_api::{
    get_order_handler, handle_pos_command, run_fiscal_signing_sweep, AppState, FiscalSettings,
};
use apex_edge_contracts::{
    AddLineItemPayload, AddPaymentPayload, ContractVersion, CreateCartPayload,
    FinalizeOrderPayload, FinalizeResult, PosCommand, PosRequestEnvelope, SetTenderingPayload,
};
use apex_edge_storage::{insert_catalog_item, insert_price_book_entry, run_migrations};
use axum::extract::State;
use axum::Json;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use uuid::Uuid;

async fn finalize_a_cart(
    state: &AppState,
    store_id: Uuid,
    register_id: Uuid,
) -> (
    bool,
    Vec<apex_edge_contracts::PosError>,
    Option<serde_json::Value>,
) {
    let item_id = Uuid::new_v4();
    insert_catalog_item(
        &state.pool,
        item_id,
        store_id,
        "FISCAL-001",
        "Fiscal Test Item",
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
    .expect("insert_catalog_item");
    insert_price_book_entry(&state.pool, store_id, item_id, None, 1_000, "EUR")
        .await
        .expect("insert_price_book_entry");

    let created = handle_pos_command(
        State(state.clone()),
        None,
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id,
            payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
        }),
    )
    .await;
    let cart_state: apex_edge_contracts::CartState =
        serde_json::from_value(created.0.payload.unwrap()).expect("cart state");

    let _ = handle_pos_command(
        State(state.clone()),
        None,
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id,
            payload: PosCommand::AddLineItem(AddLineItemPayload {
                cart_id: cart_state.cart_id,
                item_id,
                modifier_option_ids: vec![],
                quantity: 1,
                notes: None,
                unit_price_override_cents: None,
            }),
        }),
    )
    .await;
    let _ = handle_pos_command(
        State(state.clone()),
        None,
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id,
            payload: PosCommand::SetTendering(SetTenderingPayload {
                cart_id: cart_state.cart_id,
            }),
        }),
    )
    .await;
    let _ = handle_pos_command(
        State(state.clone()),
        None,
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id,
            payload: PosCommand::AddPayment(AddPaymentPayload {
                cart_id: cart_state.cart_id,
                tender_id: Uuid::new_v4(),
                amount_cents: 1_000,
                tip_amount_cents: 0,
                external_reference: Some("cash".into()),
                provider: None,
                provider_payment_id: None,
                entry_method: None,
            }),
        }),
    )
    .await;

    let finalized = handle_pos_command(
        State(state.clone()),
        None,
        Json(PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id,
            register_id,
            payload: PosCommand::FinalizeOrder(FinalizeOrderPayload {
                cart_id: cart_state.cart_id,
            }),
        }),
    )
    .await;
    (finalized.0.success, finalized.0.errors, finalized.0.payload)
}

async fn state_with_fiscal(fiscal: FiscalSettings) -> (AppState, Uuid, Uuid) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let store_id = Uuid::nil();
    let register_id = Uuid::new_v4();
    let state = AppState {
        fiscal,
        ..AppState::new(pool, store_id)
    };
    (state, store_id, register_id)
}

#[tokio::test]
async fn finalize_with_default_noop_fiscal_provider_persists_null_receipt() {
    let (state, store_id, register_id) = state_with_fiscal(FiscalSettings::default()).await;
    let (success, errors, payload) = finalize_a_cart(&state, store_id, register_id).await;
    assert!(success, "errors: {errors:?}");
    let result: FinalizeResult =
        serde_json::from_value(payload.unwrap()).expect("finalize payload");

    let order = get_order_handler(State(state), axum::extract::Path(result.order_id))
        .await
        .expect("order must exist");
    assert_eq!(order.0.fiscal_provider.as_deref(), Some("noop"));
    assert!(order.0.fiscal_id.is_none());
    assert!(order.0.fiscal_signature.is_none());
}

#[tokio::test]
async fn finalize_with_de_tse_configured_persists_fiscal_receipt() {
    let fiscal = FiscalSettings {
        provider: Arc::new(DeTseFiscalProvider::new(true)),
        currency: "EUR".into(),
    };
    let (state, store_id, register_id) = state_with_fiscal(fiscal).await;
    let (success, errors, payload) = finalize_a_cart(&state, store_id, register_id).await;
    assert!(success, "errors: {errors:?}");
    let result: FinalizeResult =
        serde_json::from_value(payload.unwrap()).expect("finalize payload");

    let order = get_order_handler(State(state), axum::extract::Path(result.order_id))
        .await
        .expect("order must exist");
    assert_eq!(order.0.fiscal_provider.as_deref(), Some("de_tse"));
    assert!(order.0.fiscal_id.is_some());
    assert!(order.0.fiscal_signature.is_some());
}

#[tokio::test]
async fn finalize_with_de_tse_unconfigured_fails_closed_and_creates_no_order() {
    let fiscal = FiscalSettings {
        provider: Arc::new(DeTseFiscalProvider::new(false)),
        currency: "EUR".into(),
    };
    let (state, store_id, register_id) = state_with_fiscal(fiscal).await;
    let (success, errors, payload) = finalize_a_cart(&state, store_id, register_id).await;

    assert!(!success, "unconfigured DE-TSE must fail closed");
    assert!(payload.is_none());
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, "FISCAL_SIGNING_FAILED");

    let orders = apex_edge_storage::list_order_ledger_entries(&state.pool, store_id, None)
        .await
        .expect("list orders");
    assert!(
        orders.is_empty(),
        "no order should be persisted when fiscal signing fails closed"
    );
}

struct TemporarilyDownTse {
    remaining_failures: AtomicU32,
}

#[async_trait::async_trait]
impl FiscalProvider for TemporarilyDownTse {
    fn provider_code(&self) -> &'static str {
        "de_tse"
    }

    fn offline_policy(&self) -> OfflinePolicy {
        OfflinePolicy::SignLater
    }

    async fn sign(&self, transaction: &FiscalTransaction) -> Result<FiscalSignature, FiscalError> {
        transaction.validate()?;
        if self.remaining_failures.load(Ordering::SeqCst) > 0 {
            self.remaining_failures.fetch_sub(1, Ordering::SeqCst);
            return Err(FiscalError::Unavailable {
                provider: "de_tse".into(),
                detail: "tse timeout".into(),
            });
        }
        Ok(FiscalSignature {
            provider: "de_tse".into(),
            fiscal_id: Some("tse_later".into()),
            signature: Some("sig_later".into()),
            qr_payload: Some(format!("V0;{}", transaction.transaction_id)),
            signed_at: chrono::Utc::now(),
        })
    }
}

struct DownAndFailClosed;

#[async_trait::async_trait]
impl FiscalProvider for DownAndFailClosed {
    fn provider_code(&self) -> &'static str {
        "strict_tse"
    }

    fn offline_policy(&self) -> OfflinePolicy {
        OfflinePolicy::FailClosed
    }

    async fn sign(&self, transaction: &FiscalTransaction) -> Result<FiscalSignature, FiscalError> {
        transaction.validate()?;
        Err(FiscalError::Unavailable {
            provider: "strict_tse".into(),
            detail: "tse timeout".into(),
        })
    }
}

#[tokio::test]
async fn a_transient_outage_lets_the_sale_complete_and_the_sweeper_signs_later() {
    let fiscal = FiscalSettings {
        provider: Arc::new(TemporarilyDownTse {
            remaining_failures: AtomicU32::new(1),
        }),
        currency: "EUR".into(),
    };
    let (state, store_id, register_id) = state_with_fiscal(fiscal).await;
    let (success, errors, payload) = finalize_a_cart(&state, store_id, register_id).await;
    assert!(success, "the till must keep selling: {errors:?}");
    let result: FinalizeResult =
        serde_json::from_value(payload.unwrap()).expect("finalize payload");

    let pending = get_order_handler(State(state.clone()), axum::extract::Path(result.order_id))
        .await
        .expect("order must exist");
    assert!(
        pending.0.fiscal_pending,
        "the sale is on the ledger, waiting for a signature"
    );
    assert!(pending.0.fiscal_id.is_none());
    assert_eq!(
        apex_edge_storage::count_fiscal_queue(
            &state.pool,
            apex_edge_storage::FiscalQueueStatus::Pending
        )
        .await
        .expect("count"),
        1
    );

    let signed = run_fiscal_signing_sweep(&state.pool, &state.fiscal)
        .await
        .expect("sweep");
    assert_eq!(signed, 1);

    let order = get_order_handler(State(state.clone()), axum::extract::Path(result.order_id))
        .await
        .expect("order still exists");
    assert!(!order.0.fiscal_pending);
    assert_eq!(order.0.fiscal_id.as_deref(), Some("tse_later"));
    assert_eq!(order.0.fiscal_signature.as_deref(), Some("sig_later"));
    assert_eq!(
        apex_edge_storage::count_fiscal_queue(
            &state.pool,
            apex_edge_storage::FiscalQueueStatus::Pending
        )
        .await
        .expect("count"),
        0
    );
}

#[tokio::test]
async fn a_fail_closed_jurisdiction_does_not_complete_an_unsigned_sale() {
    let fiscal = FiscalSettings {
        provider: Arc::new(DownAndFailClosed),
        currency: "EUR".into(),
    };
    let (state, store_id, register_id) = state_with_fiscal(fiscal).await;
    let (success, errors, payload) = finalize_a_cart(&state, store_id, register_id).await;

    assert!(!success);
    assert!(payload.is_none());
    assert_eq!(errors[0].code, "FISCAL_SIGNING_FAILED");
    let orders = apex_edge_storage::list_order_ledger_entries(&state.pool, store_id, None)
        .await
        .expect("list orders");
    assert!(orders.is_empty());
}
