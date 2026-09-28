//! VAT-inclusive pricing.
//!
//! In inclusive jurisdictions (most of the EU) the shelf price already contains the tax: an
//! item priced 11.00 with 10% VAT costs the customer 11.00, of which 1.00 is tax. The hub
//! must extract the tax, not add it on top, and every downstream record (cart, order
//! ledger, fiscal transaction) must agree on net 10.00 + tax 1.00 = gross 11.00.

use std::sync::{Arc, Mutex};

use apex_edge_adapters_fiscal::{
    FiscalError, FiscalProvider, FiscalSignature, FiscalTransaction, OfflinePolicy,
};
use apex_edge_api::{pos_handler::execute_pos_command, AppState, FiscalSettings};
use apex_edge_contracts::{
    AddLineItemPayload, AddPaymentPayload, ContractVersion, CreateCartPayload,
    FinalizeOrderPayload, PosCommand, PosRequestEnvelope, PosResponseEnvelope, SetTenderingPayload,
};
use apex_edge_storage::{
    create_sqlite_pool, fetch_order_ledger_entry, insert_catalog_item, insert_price_book_entry,
    insert_tax_rule, run_migrations, set_audit_key, AuditKey,
};
use serde_json::Value;
use uuid::Uuid;

const STORE: Uuid = Uuid::nil();

/// Records the transaction it was asked to sign.
#[derive(Default)]
struct RecordingFiscal {
    signed: Mutex<Vec<FiscalTransaction>>,
}

#[async_trait::async_trait]
impl FiscalProvider for RecordingFiscal {
    fn provider_code(&self) -> &'static str {
        "recording"
    }

    fn offline_policy(&self) -> OfflinePolicy {
        OfflinePolicy::FailClosed
    }

    async fn sign(&self, transaction: &FiscalTransaction) -> Result<FiscalSignature, FiscalError> {
        transaction.validate()?;
        self.signed.lock().unwrap().push(transaction.clone());
        Ok(FiscalSignature {
            provider: "recording".into(),
            fiscal_id: Some("f-1".into()),
            signature: Some("sig".into()),
            qr_payload: None,
            signed_at: chrono::Utc::now(),
        })
    }
}

struct Hub {
    state: AppState,
    fiscal: Arc<RecordingFiscal>,
    inclusive_item: Uuid,
    exclusive_item: Uuid,
}

async fn hub() -> Hub {
    set_audit_key(AuditKey::new("test-hub", b"test-secret".to_vec()));
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();

    let vat = Uuid::new_v4();
    let sales_tax = Uuid::new_v4();
    insert_tax_rule(&pool, Uuid::new_v4(), STORE, vat, 1000, "VAT 10%", true)
        .await
        .unwrap();
    insert_tax_rule(
        &pool,
        Uuid::new_v4(),
        STORE,
        sales_tax,
        1000,
        "Sales tax 10%",
        false,
    )
    .await
    .unwrap();

    let inclusive_item = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        inclusive_item,
        STORE,
        "EU-1",
        "Espresso",
        Uuid::new_v4(),
        vat,
    )
    .await
    .unwrap();
    insert_price_book_entry(&pool, STORE, inclusive_item, None, 1100, "EUR")
        .await
        .unwrap();

    let exclusive_item = Uuid::new_v4();
    insert_catalog_item(
        &pool,
        exclusive_item,
        STORE,
        "US-1",
        "Coffee",
        Uuid::new_v4(),
        sales_tax,
    )
    .await
    .unwrap();
    insert_price_book_entry(&pool, STORE, exclusive_item, None, 1000, "EUR")
        .await
        .unwrap();

    let fiscal = Arc::new(RecordingFiscal::default());
    let state = AppState {
        fiscal: FiscalSettings {
            provider: fiscal.clone(),
            currency: "EUR".into(),
        },
        ..AppState::new(pool, STORE)
    };
    Hub {
        state,
        fiscal,
        inclusive_item,
        exclusive_item,
    }
}

async fn send(hub: &Hub, command: PosCommand) -> PosResponseEnvelope<Value> {
    execute_pos_command(
        &hub.state,
        PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id: STORE,
            register_id: Uuid::nil(),
            payload: command,
        },
    )
    .await
}

async fn cart_with(hub: &Hub, item_id: Uuid) -> (Uuid, Value) {
    let cart_id = Uuid::new_v4();
    assert!(
        send(
            hub,
            PosCommand::CreateCart(CreateCartPayload {
                cart_id: Some(cart_id)
            })
        )
        .await
        .success
    );
    let added = send(
        hub,
        PosCommand::AddLineItem(AddLineItemPayload {
            cart_id,
            item_id,
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }),
    )
    .await;
    assert!(added.success, "{:?}", added.errors);
    (cart_id, added.payload.expect("cart"))
}

async fn pay_exactly_and_finalize(hub: &Hub, cart_id: Uuid, amount_cents: u64) -> Uuid {
    assert!(
        send(
            hub,
            PosCommand::SetTendering(SetTenderingPayload { cart_id })
        )
        .await
        .success
    );
    let paid = send(
        hub,
        PosCommand::AddPayment(AddPaymentPayload {
            cart_id,
            tender_id: Uuid::new_v4(),
            amount_cents,
            tip_amount_cents: 0,
            external_reference: Some("cash".into()),
            provider: None,
            provider_payment_id: None,
            entry_method: None,
        }),
    )
    .await;
    assert!(paid.success, "{:?}", paid.errors);
    let done = send(
        hub,
        PosCommand::FinalizeOrder(FinalizeOrderPayload { cart_id }),
    )
    .await;
    assert!(done.success, "{:?}", done.errors);
    done.payload.expect("finalize")["order_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn an_inclusive_price_is_what_the_customer_pays_with_the_tax_extracted() {
    let hub = hub().await;
    let (_, cart) = cart_with(&hub, hub.inclusive_item).await;

    assert_eq!(cart["subtotal_cents"], 1100);
    assert_eq!(cart["tax_cents"], 100, "10% VAT inside 11.00 is 1.00");
    assert_eq!(cart["total_cents"], 1100, "tax is not added on top");
    assert_eq!(cart["lines"][0]["tax_inclusive"], true);
}

#[tokio::test]
async fn an_exclusive_price_still_has_tax_added_on_top() {
    let hub = hub().await;
    let (_, cart) = cart_with(&hub, hub.exclusive_item).await;

    assert_eq!(cart["subtotal_cents"], 1000);
    assert_eq!(cart["tax_cents"], 100);
    assert_eq!(cart["total_cents"], 1100);
    assert_eq!(cart["lines"][0]["tax_inclusive"], false);
}

#[tokio::test]
async fn an_inclusive_sale_is_ledgered_and_fiscalised_as_net_plus_tax_equals_shelf_price() {
    let hub = hub().await;
    let (cart_id, _) = cart_with(&hub, hub.inclusive_item).await;
    let order_id = pay_exactly_and_finalize(&hub, cart_id, 1100).await;

    let ledger = fetch_order_ledger_entry(&hub.state.pool, order_id)
        .await
        .unwrap()
        .expect("order");
    assert_eq!(ledger.total_cents, 1100);
    assert_eq!(ledger.tax_cents, 100);
    assert!(ledger.lines[0].tax_inclusive);

    let signed = hub.fiscal.signed.lock().unwrap();
    let tx = signed.last().expect("fiscalised");
    assert_eq!(tx.gross_cents, 1100);
    assert_eq!(tx.net_cents, 1000);
    assert_eq!(tx.tax_cents, 100);
    assert!(tx.lines[0].tax_inclusive);
    assert_eq!(tx.lines[0].gross_cents, 1100);
}

#[tokio::test]
async fn a_manual_discount_on_an_inclusive_line_extracts_tax_from_the_discounted_price() {
    let hub = hub().await;
    let (cart_id, _) = cart_with(&hub, hub.inclusive_item).await;

    let discounted = send(
        &hub,
        PosCommand::ApplyManualDiscount(apex_edge_contracts::ApplyManualDiscountPayload {
            cart_id,
            reason: "loyal customer".into(),
            kind: apex_edge_contracts::ManualDiscountKind::FixedCart,
            value: 550,
            line_id: None,
        }),
    )
    .await;
    assert!(discounted.success, "{:?}", discounted.errors);
    let cart = discounted.payload.expect("cart");
    assert_eq!(cart["total_cents"], 550, "half price, still tax-inclusive");
    assert_eq!(cart["tax_cents"], 50, "VAT inside 5.50 is 0.50");
}

#[tokio::test]
async fn returning_an_inclusive_line_refunds_the_shelf_price_not_price_plus_tax() {
    let hub = hub().await;
    let started = send(
        &hub,
        PosCommand::StartReturn(apex_edge_contracts::StartReturnPayload {
            return_id: None,
            original_order_id: Some(Uuid::new_v4()),
            reason_code: Some("damaged".into()),
            approval_id: None,
            shift_id: None,
        }),
    )
    .await;
    assert!(started.success, "{:?}", started.errors);
    let return_id: Uuid = started.payload.unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let line = send(
        &hub,
        PosCommand::ReturnLineItem(apex_edge_contracts::ReturnLineItemPayload {
            return_id,
            sku: "EU-1".into(),
            name: Some("Espresso".into()),
            quantity: 1,
            unit_price_cents: 1100,
            tax_cents: 100,
            tax_inclusive: true,
            original_line_id: None,
        }),
    )
    .await;
    assert!(line.success, "{:?}", line.errors);
    let ret = line.payload.unwrap();
    assert_eq!(ret["total_cents"], 1100);
    assert_eq!(ret["tax_cents"], 100);

    let refunded = send(
        &hub,
        PosCommand::RefundTender(apex_edge_contracts::RefundTenderPayload {
            return_id,
            tender_type: "cash".into(),
            amount_cents: 1100,
            external_reference: None,
        }),
    )
    .await;
    assert_eq!(refunded.payload.unwrap()["state"], "paid");

    let done = send(
        &hub,
        PosCommand::FinalizeReturn(apex_edge_contracts::FinalizeReturnPayload { return_id }),
    )
    .await;
    assert!(done.success, "{:?}", done.errors);
    let signed = hub.fiscal.signed.lock().unwrap();
    let tx = signed.last().expect("refund fiscalised");
    assert_eq!(tx.gross_cents, -1100);
    assert_eq!(tx.net_cents, -1000);
    assert_eq!(tx.tax_cents, -100);
}
