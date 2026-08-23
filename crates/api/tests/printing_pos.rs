//! Printing behaviour at the POS command level.
//!
//! A receipt printer is the one peripheral a customer notices. These tests use the
//! in-repo `CaptureSink`, so they assert on the exact bytes the hub would have put on
//! the wire, with no printer present.

use apex_edge_adapters_hardware::codepage;
use apex_edge_api::{hardware::HardwareSettings, pos_handler::execute_pos_command, AppState};
use apex_edge_contracts::{
    AddLineItemPayload, AddPaymentPayload, ContractVersion, CreateCartPayload,
    FinalizeOrderPayload, PosCommand, PosRequestEnvelope, PosResponseEnvelope,
    PrintDocumentPayload, SetTenderingPayload,
};
use apex_edge_storage::{insert_catalog_item, insert_price_book_entry, run_migrations};
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

struct Fixture {
    state: AppState,
    sink: apex_edge_adapters_hardware::CaptureSink,
    store_id: Uuid,
    register_id: Uuid,
    item_id: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_hardware(|settings| settings).await
    }

    async fn with_hardware(adjust: impl FnOnce(HardwareSettings) -> HardwareSettings) -> Self {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("pool");
        run_migrations(&pool).await.expect("migrations");
        let store_id = Uuid::nil();
        let item_id = Uuid::new_v4();
        insert_catalog_item(
            &pool,
            item_id,
            store_id,
            "SKU-1",
            "Filter Coffee",
            Uuid::new_v4(),
            Uuid::new_v4(),
        )
        .await
        .expect("insert_catalog_item");
        insert_price_book_entry(&pool, store_id, item_id, None, 2_500, "EUR")
            .await
            .expect("insert_price_book_entry");

        let (hardware, sink) = HardwareSettings::capturing(32);
        let mut state = AppState::new(pool, store_id);
        state.hardware = adjust(hardware);
        Self {
            state,
            sink,
            store_id,
            register_id: Uuid::new_v4(),
            item_id,
        }
    }

    async fn send(&self, command: PosCommand) -> PosResponseEnvelope<Value> {
        execute_pos_command(
            &self.state,
            PosRequestEnvelope {
                version: ContractVersion::V1_0_0,
                idempotency_key: Uuid::new_v4(),
                store_id: self.store_id,
                register_id: self.register_id,
                payload: command,
            },
        )
        .await
    }

    /// Runs a whole sale and returns the finalize response.
    async fn sell(&self, tender: Tender) -> PosResponseEnvelope<Value> {
        let cart_id = Uuid::new_v4();
        self.send(PosCommand::CreateCart(CreateCartPayload {
            cart_id: Some(cart_id),
        }))
        .await;
        self.send(PosCommand::AddLineItem(AddLineItemPayload {
            cart_id,
            item_id: self.item_id,
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }))
        .await;
        self.send(PosCommand::SetTendering(SetTenderingPayload { cart_id }))
            .await;
        self.send(PosCommand::AddPayment(AddPaymentPayload {
            cart_id,
            tender_id: Uuid::new_v4(),
            amount_cents: 2_500,
            tip_amount_cents: 0,
            external_reference: Some(tender.reference().into()),
            provider: None,
            provider_payment_id: None,
            entry_method: None,
        }))
        .await;
        self.send(PosCommand::FinalizeOrder(FinalizeOrderPayload { cart_id }))
            .await
    }

    /// Everything the printer was told to print, decoded back to readable text.
    fn printed_text(&self) -> String {
        self.sink
            .jobs()
            .iter()
            .map(|job| codepage::decode(job))
            .collect()
    }
}

#[derive(Clone, Copy)]
enum Tender {
    Cash,
    Card,
}

impl Tender {
    fn reference(self) -> &'static str {
        match self {
            Tender::Cash => "cash",
            Tender::Card => "auth-90210",
        }
    }
}

#[tokio::test]
async fn finalizing_a_sale_prints_the_receipt() {
    let f = Fixture::new().await;

    let response = f.sell(Tender::Cash).await;

    assert!(response.success, "{:?}", response.errors);
    let printed = f.printed_text();
    assert!(
        printed.contains("Filter Coffee"),
        "the receipt must list what was bought: {printed}"
    );
    assert!(
        printed.contains("25.00"),
        "the receipt must show the price: {printed}"
    );
}

#[tokio::test]
async fn the_receipt_shows_totals_and_how_the_customer_paid() {
    let f = Fixture::new().await;

    f.sell(Tender::Cash).await;

    let printed = f.printed_text();
    for expected in ["TOTAL", "Subtotal", "CASH"] {
        assert!(
            printed.contains(expected),
            "missing {expected} in receipt: {printed}"
        );
    }
}

#[tokio::test]
async fn a_cash_sale_opens_the_drawer() {
    let f = Fixture::new().await;

    f.sell(Tender::Cash).await;

    let kicks = f
        .sink
        .jobs()
        .iter()
        .filter(|job| job.as_slice() == [0x1B, 0x70, 0x00, 0x19, 0xFA])
        .count();
    assert_eq!(kicks, 1, "a cash sale must open the drawer exactly once");
}

#[tokio::test]
async fn a_card_sale_leaves_the_drawer_shut() {
    // Opening the drawer on every card sale is both a security problem and the fastest
    // way to make staff stop trusting the till.
    let f = Fixture::new().await;

    f.sell(Tender::Card).await;

    assert!(
        !f.sink
            .jobs()
            .iter()
            .any(|job| job.as_slice() == [0x1B, 0x70, 0x00, 0x19, 0xFA]),
        "a card sale must not open the drawer"
    );
}

#[tokio::test]
async fn a_hub_with_no_printer_still_completes_the_sale() {
    // The overwhelmingly common configuration: no printer wired to the hub, the POS
    // fetches the document itself. That path must not regress.
    let f = Fixture::with_hardware(|_| HardwareSettings::default()).await;

    let response = f.sell(Tender::Cash).await;

    assert!(response.success, "{:?}", response.errors);
    let payload = response.payload.expect("finalize payload");
    assert!(payload["print_error"].is_null());
    assert!(f.sink.jobs().is_empty());
}

#[tokio::test]
async fn a_printer_that_fails_reports_the_problem_without_losing_the_sale() {
    // The money is already taken and the order is already durable. Failing the command
    // now would tell the operator the sale did not happen, which is a lie.
    let f = Fixture::with_hardware(|settings| HardwareSettings {
        device: Some(std::sync::Arc::new(
            apex_edge_adapters_hardware::TransportPrinter::new(
                apex_edge_adapters_hardware::EscPosEncoder,
                apex_edge_adapters_hardware::Tcp9100Transport::new("127.0.0.1:1"),
            ),
        )),
        ..settings
    })
    .await;

    let response = f.sell(Tender::Cash).await;

    assert!(response.success, "{:?}", response.errors);
    let payload = response.payload.expect("finalize payload");
    assert!(
        payload["print_error"]
            .as_str()
            .expect("a failed print must be reported")
            .contains("tcp9100"),
        "the operator needs to know which device failed: {payload}"
    );
    assert!(
        !payload["print_job_ids"]
            .as_array()
            .expect("print jobs")
            .is_empty(),
        "the document still exists and can be reprinted"
    );
}

#[tokio::test]
async fn a_document_can_be_reprinted_on_demand() {
    let f = Fixture::new().await;
    let finalize = f.sell(Tender::Card).await;
    let document_id: Uuid =
        serde_json::from_value(finalize.payload.expect("payload")["print_job_ids"][0].clone())
            .expect("document id");
    f.sink.clear();

    let response = f
        .send(PosCommand::PrintDocument(PrintDocumentPayload {
            document_id,
            open_drawer: false,
        }))
        .await;

    assert!(response.success, "{:?}", response.errors);
    assert_eq!(
        response.payload.expect("payload")["encoder"],
        Value::String("escpos".into())
    );
    assert!(f.printed_text().contains("Filter Coffee"));
}

#[tokio::test]
async fn a_reprint_can_be_asked_to_open_the_drawer() {
    // The manager override: a till that needs opening without a sale.
    let f = Fixture::new().await;
    let finalize = f.sell(Tender::Card).await;
    let document_id: Uuid =
        serde_json::from_value(finalize.payload.expect("payload")["print_job_ids"][0].clone())
            .expect("document id");
    f.sink.clear();

    f.send(PosCommand::PrintDocument(PrintDocumentPayload {
        document_id,
        open_drawer: true,
    }))
    .await;

    assert!(f
        .sink
        .jobs()
        .iter()
        .any(|job| job.as_slice() == [0x1B, 0x70, 0x00, 0x19, 0xFA]));
}

#[tokio::test]
async fn reprinting_a_document_that_does_not_exist_is_refused() {
    let f = Fixture::new().await;

    let response = f
        .send(PosCommand::PrintDocument(PrintDocumentPayload {
            document_id: Uuid::new_v4(),
            open_drawer: false,
        }))
        .await;

    assert!(!response.success);
    assert_eq!(response.errors[0].code, "DOCUMENT_NOT_FOUND");
    assert!(f.sink.jobs().is_empty());
}

#[tokio::test]
async fn asking_a_hub_with_no_printer_to_print_is_refused_rather_than_ignored() {
    let f = Fixture::with_hardware(|_| HardwareSettings::default()).await;
    let finalize = f.sell(Tender::Card).await;
    let document_id: Uuid =
        serde_json::from_value(finalize.payload.expect("payload")["print_job_ids"][0].clone())
            .expect("document id");

    let response = f
        .send(PosCommand::PrintDocument(PrintDocumentPayload {
            document_id,
            open_drawer: false,
        }))
        .await;

    assert!(!response.success);
    assert_eq!(response.errors[0].code, "PRINTER_NOT_CONFIGURED");
}

#[tokio::test]
async fn a_star_printer_receives_star_commands_not_epson_ones() {
    // Same receipt, different dialect: this is the check that the configured encoder is
    // actually the one used, rather than a default sneaking through.
    let (hardware, sink) = HardwareSettings::capturing_star(32);
    let f = Fixture::with_hardware(|_| hardware).await;

    f.sell(Tender::Cash).await;

    let jobs = sink.jobs();
    assert!(!jobs.is_empty(), "nothing was printed");
    assert!(
        jobs[0].windows(4).any(|w| w == [0x1B, 0x1D, 0x74, 0x00]),
        "expected the Star code page command, got {:02X?}",
        &jobs[0][..jobs[0].len().min(16)]
    );
    assert!(
        jobs.iter()
            .any(|job| job.as_slice() == [0x1B, 0x07, 0x05, 0x32, 0x07]),
        "the drawer kick must use the Star dialect too"
    );
}
