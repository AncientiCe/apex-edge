//! Behavioural coverage for DSFinV-K cash-point closing files and EN 16931
//! invoices (XRechnung UBL and Factur-X CII).

use apex_edge_adapters_fiscal::{
    export::{
        cii_factur_x, dsfinvk_files, evaluate_schematron, ubl_xrechnung, CashPointClosing,
        En16931Profile,
    },
    CashPoint, FiscalLine, FiscalTender, FiscalTenderType, FiscalTransaction,
    FiscalTransactionKind,
};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

fn sale() -> FiscalTransaction {
    let net = 1000i64;
    let tax = 190i64;
    FiscalTransaction {
        transaction_id: Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap(),
        kind: FiscalTransactionKind::Sale,
        reference_transaction_id: None,
        cash_point: CashPoint {
            store_id: Uuid::nil(),
            register_id: Uuid::parse_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap(),
            shift_id: Some(Uuid::nil()),
            operator_id: None,
        },
        occurred_at: Utc.with_ymd_and_hms(2026, 3, 4, 10, 30, 0).unwrap(),
        currency: "EUR".into(),
        lines: vec![FiscalLine {
            line_id: Uuid::nil(),
            sku: "SKU-1".into(),
            description: "Coffee & cake".into(),
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

fn closing() -> CashPointClosing {
    CashPointClosing {
        shift_id: Uuid::nil(),
        store_id: Uuid::nil(),
        register_id: Uuid::parse_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap(),
        opened_at: Utc.with_ymd_and_hms(2026, 3, 4, 8, 0, 0).unwrap(),
        closed_at: Utc.with_ymd_and_hms(2026, 3, 4, 18, 0, 0).unwrap(),
        opening_float_cents: 5_000,
        counted_cents: 6_190,
        expected_cents: 6_190,
        currency: "EUR".into(),
        tax_id: "DE123456789".into(),
        transactions: vec![sale()],
    }
}

#[test]
fn dsfinvk_emits_the_cash_register_the_receipts_and_the_vat_breakdown() {
    let files = dsfinvk_files(&closing());
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"Stamm_Kassen.csv"));
    assert!(names.contains(&"Bonkopf.csv"));
    assert!(names.contains(&"Bonpos.csv"));
    assert!(names.contains(&"Bonpos_USt.csv"));
    assert!(names.contains(&"Z_Zahlart.csv"));

    let kassen = files
        .iter()
        .find(|(n, _)| n == "Stamm_Kassen.csv")
        .map(|(_, c)| c.as_str())
        .unwrap();
    assert!(kassen.contains("DE123456789"));
    assert!(kassen.contains(';'), "DSFinV-K is semicolon-separated");

    let heads = files
        .iter()
        .find(|(n, _)| n == "Bonkopf.csv")
        .map(|(_, c)| c.as_str())
        .unwrap();
    assert!(heads.contains("11111111-2222-3333-4444-555555555555"));
    assert!(heads.contains("11.90") || heads.contains("1190"));

    let vat = files
        .iter()
        .find(|(n, _)| n == "Bonpos_USt.csv")
        .map(|(_, c)| c.as_str())
        .unwrap();
    assert!(vat.contains("19.00") || vat.contains("1900"));
}

#[test]
fn dsfinvk_escapes_semicolons_and_ampersands_in_item_names() {
    let files = dsfinvk_files(&closing());
    let lines = files
        .iter()
        .find(|(n, _)| n == "Bonpos.csv")
        .map(|(_, c)| c.as_str())
        .unwrap();
    assert!(
        lines.contains("Coffee") && !lines.contains("Coffee & cake;"),
        "a semicolon in a description must not create an extra column: {lines}"
    );
}

#[test]
fn xrechnung_ubl_carries_the_en16931_customization_and_the_vat() {
    let xml = ubl_xrechnung(&sale(), "DE123456789", "Cafe Beispiel");
    assert!(xml.contains("urn:cen.eu:en16931:2017"));
    assert!(xml.contains("xrechnung"));
    assert!(xml.contains("<cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>"));
    assert!(xml.contains("<cbc:ID>"));
    assert!(xml.contains("2026-03-04"));
    assert!(xml.contains("Coffee"));
    assert!(xml.contains("&amp;"), "XML must escape the item name");
    assert!(xml.contains("TaxTotal") || xml.contains("cac:TaxTotal"));
}

#[test]
fn factur_x_cii_uses_the_cross_industry_invoice_root() {
    let xml = cii_factur_x(&sale(), "DE123456789", "Cafe Beispiel");
    assert!(xml.contains("CrossIndustryInvoice"));
    assert!(xml.contains("factur-x") || xml.contains("en16931"));
    assert!(xml.contains("Coffee"));
}

#[test]
fn xrechnung_passes_the_embedded_schematron_rules() {
    let xml = ubl_xrechnung(&sale(), "DE123456789", "Cafe Beispiel");
    let sch = include_str!("schematron/xrechnung.sch");
    let failures = evaluate_schematron(xml.as_str(), sch);
    assert!(
        failures.is_empty(),
        "XRechnung Schematron failed: {failures:?}\n{xml}"
    );
}

#[test]
fn factur_x_passes_the_embedded_schematron_rules() {
    let xml = cii_factur_x(&sale(), "DE123456789", "Cafe Beispiel");
    let sch = include_str!("schematron/factur-x.sch");
    let failures = evaluate_schematron(xml.as_str(), sch);
    assert!(
        failures.is_empty(),
        "Factur-X Schematron failed: {failures:?}\n{xml}"
    );
}

#[test]
fn schematron_rejects_an_invoice_that_forgot_its_vat() {
    let xml = "<Invoice xmlns:cbc=\"urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2\"><cbc:ID>1</cbc:ID></Invoice>";
    let sch = include_str!("schematron/xrechnung.sch");
    let failures = evaluate_schematron(xml, sch);
    assert!(
        !failures.is_empty(),
        "a document missing required BT fields must not pass"
    );
}

#[test]
fn profile_names_are_stable() {
    assert_eq!(En16931Profile::XRechnung.as_str(), "xrechnung");
    assert_eq!(En16931Profile::FacturX.as_str(), "factur-x");
}
