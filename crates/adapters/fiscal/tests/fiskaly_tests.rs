//! Behavioural coverage for Fiskaly SIGN (DE sandbox) and the RKSV / VeriFactu
//! receipt QR payloads that Austria and Spain require on the paper.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use apex_edge_adapters_fiscal::{
    qr, CashPoint, FiscalError, FiscalLine, FiscalProvider, FiscalTender, FiscalTenderType,
    FiscalTransaction, FiscalTransactionKind, FiskalyConfig, FiskalyMarket, FiskalyProvider,
};
use axum::extract::State;
use axum::routing::{post, put};
use axum::{Json, Router};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

fn cash_point() -> CashPoint {
    CashPoint {
        store_id: Uuid::nil(),
        register_id: Uuid::parse_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap(),
        shift_id: Some(Uuid::nil()),
        operator_id: None,
    }
}

fn sale() -> FiscalTransaction {
    let net = 1000i64;
    let tax = 190i64;
    FiscalTransaction {
        transaction_id: Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap(),
        kind: FiscalTransactionKind::Sale,
        reference_transaction_id: None,
        cash_point: cash_point(),
        occurred_at: Utc.with_ymd_and_hms(2026, 3, 4, 10, 30, 0).unwrap(),
        currency: "EUR".into(),
        lines: vec![FiscalLine {
            line_id: Uuid::nil(),
            sku: "SKU-1".into(),
            description: "Coffee".into(),
            quantity: 1,
            unit_price_cents: 1190,
            net_cents: net,
            tax_cents: tax,
            gross_cents: net + tax,
            discount_cents: 0,
            tax_rate_bps: 1900,
            tax_category: "standard".into(),
            tax_inclusive: false,
        }],
        tenders: vec![FiscalTender {
            tender_id: Uuid::nil(),
            tender_type: FiscalTenderType::Cash,
            amount_cents: net + tax,
            tip_cents: 0,
            provider: None,
            provider_reference: None,
        }],
        net_cents: net,
        tax_cents: tax,
        gross_cents: net + tax,
    }
}

#[derive(Clone, Default)]
struct FakeFiskaly {
    calls: Arc<Mutex<Vec<String>>>,
    fail_auth: Arc<Mutex<bool>>,
    fail_finish: Arc<Mutex<bool>>,
}

impl FakeFiskaly {
    fn record(&self, path: &str) {
        self.calls.lock().expect("calls").push(path.into());
    }
}

async fn auth(
    State(state): State<FakeFiskaly>,
    body: String,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    state.record("auth");
    assert!(
        body.contains("api_key"),
        "fiskaly auth sends the api key, got {body}"
    );
    if *state.fail_auth.lock().expect("fail_auth") {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"message": "bad credentials"})),
        );
    }
    (
        axum::http::StatusCode::OK,
        Json(serde_json::json!({"access_token": "tok_test"})),
    )
}

async fn put_tx(
    State(state): State<FakeFiskaly>,
    axum::extract::Path((tss, tx)): axum::extract::Path<(String, String)>,
) -> Json<serde_json::Value> {
    state.record(&format!("put:{tss}:{tx}"));
    Json(serde_json::json!({"state": "ACTIVE"}))
}

async fn patch_tx(
    State(state): State<FakeFiskaly>,
    axum::extract::Path((tss, tx)): axum::extract::Path<(String, String)>,
    body: String,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    state.record(&format!("patch:{tss}:{tx}"));
    assert!(
        body.contains("FINISHED"),
        "the receipt is only signed when the transaction is finished, got {body}"
    );
    assert!(
        body.contains("NORMAL") || body.contains("amounts_per_vat_rate"),
        "fiskaly needs the VAT breakdown, got {body}"
    );
    if *state.fail_finish.lock().expect("fail_finish") {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"message": "tss timeout"})),
        );
    }
    (
        axum::http::StatusCode::OK,
        Json(serde_json::json!({
            "number": 42,
            "qr_code_data": "V0;TSS;sig",
            "log": { "signature": { "value": "remote-sig" }, "timestamp": "2026-03-04T10:30:01Z" }
        })),
    )
}

async fn serve(fake: FakeFiskaly) -> (SocketAddr, FakeFiskaly) {
    let app = Router::new()
        .route("/auth", post(auth))
        .route("/tss/:tss_id/tx/:tx_id", put(put_tx).patch(patch_tx))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (addr, fake)
}

fn config(addr: SocketAddr, market: FiskalyMarket) -> FiskalyConfig {
    FiskalyConfig {
        api_key: "key".into(),
        api_secret: "secret".into(),
        tss_id: "tss-1".into(),
        client_id: "client-1".into(),
        tax_id: "ATU12345678".into(),
        market,
        api_base_url: format!("http://{addr}"),
    }
}

#[tokio::test]
async fn missing_credentials_fail_closed() {
    let provider = FiskalyProvider::new(FiskalyConfig {
        api_key: String::new(),
        api_secret: String::new(),
        tss_id: "tss".into(),
        client_id: "client".into(),
        tax_id: String::new(),
        market: FiskalyMarket::De,
        api_base_url: "https://kassensichv.io/api/v2".into(),
    })
    .expect("an unconfigured provider still constructs so the hub can fail closed at sign time");
    let err = provider.sign(&sale()).await.expect_err("empty credentials");
    assert!(matches!(err, FiscalError::NotConfigured { .. }));
    assert!(!err.is_transient());
}

#[test]
fn every_supported_market_has_a_stable_code() {
    for market in FiskalyMarket::ALL {
        assert!(!market.as_str().is_empty());
        assert_eq!(FiskalyMarket::parse(market.as_str()), Some(*market));
    }
}

#[test]
fn the_rksv_qr_identifies_the_register_the_receipt_and_the_vat_totals() {
    let qr = qr::rksv_payload(&sale(), "ATU12345678");
    assert!(
        qr.starts_with("_R1-AT1_"),
        "RKSV machine-readable code: {qr}"
    );
    assert!(
        qr.contains("aaaaaaaa"),
        "cash-register id is in the code: {qr}"
    );
    assert!(
        qr.contains("11111111"),
        "receipt number is the transaction: {qr}"
    );
    assert!(
        qr.contains("11.90") || qr.contains("1190"),
        "gross amount is in the code: {qr}"
    );
}

#[test]
fn the_verifactu_qr_is_an_aeat_validation_url() {
    let qr = qr::verifactu_payload(&sale(), "B12345678");
    assert!(
        qr.starts_with("https://www2.agenciatributaria.gob.es/"),
        "{qr}"
    );
    assert!(qr.contains("nif=B12345678"), "{qr}");
    assert!(qr.contains("importe="), "{qr}");
    assert!(qr.contains("fecha=04-03-2026"), "{qr}");
}

#[tokio::test]
async fn fiskaly_signs_a_german_sale_against_the_sandbox_shaped_api() {
    let (addr, fake) = serve(FakeFiskaly::default()).await;
    let provider = FiskalyProvider::new(config(addr, FiskalyMarket::De)).expect("configured");
    let signature = provider.sign(&sale()).await.expect("sign");

    assert_eq!(signature.provider, "fiskaly");
    assert_eq!(signature.fiscal_id.as_deref(), Some("42"));
    assert_eq!(signature.signature.as_deref(), Some("remote-sig"));
    assert_eq!(signature.qr_payload.as_deref(), Some("V0;TSS;sig"));
    let calls = fake.calls.lock().expect("calls").clone();
    assert!(calls.iter().any(|c| c == "auth"));
    assert!(calls.iter().any(|c| c.starts_with("put:")));
    assert!(calls.iter().any(|c| c.starts_with("patch:")));
}

#[tokio::test]
async fn an_austrian_sale_prints_an_rksv_qr_even_when_fiskaly_also_signs() {
    let (addr, _) = serve(FakeFiskaly::default()).await;
    let provider = FiskalyProvider::new(config(addr, FiskalyMarket::At)).expect("configured");
    let signature = provider.sign(&sale()).await.expect("sign");
    let qr = signature.qr_payload.expect("qr");
    assert!(qr.starts_with("_R1-AT1_"), "{qr}");
}

#[tokio::test]
async fn a_spanish_sale_prints_a_verifactu_qr() {
    let (addr, _) = serve(FakeFiskaly::default()).await;
    let mut cfg = config(addr, FiskalyMarket::Es);
    cfg.tax_id = "B12345678".into();
    let provider = FiskalyProvider::new(cfg).expect("configured");
    let signature = provider.sign(&sale()).await.expect("sign");
    let qr = signature.qr_payload.expect("qr");
    assert!(qr.contains("agenciatributaria.gob.es"), "{qr}");
}

#[tokio::test]
async fn a_down_sandbox_is_a_transient_error_so_the_till_can_sign_later() {
    let fake = FakeFiskaly::default();
    *fake.fail_finish.lock().expect("fail") = true;
    let (addr, _) = serve(fake).await;
    let provider = FiskalyProvider::new(config(addr, FiskalyMarket::De)).expect("configured");
    let err = provider.sign(&sale()).await.expect_err("outage");
    assert!(err.is_transient(), "{err}");
}

#[tokio::test]
async fn rejected_credentials_are_permanent() {
    let fake = FakeFiskaly::default();
    *fake.fail_auth.lock().expect("fail") = true;
    let (addr, _) = serve(fake).await;
    let provider = FiskalyProvider::new(config(addr, FiskalyMarket::De)).expect("configured");
    let err = provider.sign(&sale()).await.expect_err("auth");
    assert!(!err.is_transient(), "{err}");
}
