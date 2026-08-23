//! Behavioural coverage for the fiscal transaction contract.
//!
//! A fiscal authority does not care about an order total; it cares about what was sold,
//! at which tax rate, how it was paid for, on which cash point, and when. These tests
//! pin down that shape: a transaction that does not reconcile is refused before it can
//! reach a provider, a provider reports whether a failure is worth retrying, and both
//! reference providers speak the same contract for sales and refunds.

use apex_edge_adapters_fiscal::{
    CashPoint, DeTseFiscalProvider, FiscalError, FiscalLine, FiscalProvider, FiscalTender,
    FiscalTenderType, FiscalTransaction, FiscalTransactionKind, NoOpFiscalProvider, OfflinePolicy,
};
use chrono::{TimeZone, Utc};
use uuid::Uuid;

/// A 19% standard-rate line: 10.00 net, 1.90 tax, 11.90 gross.
fn standard_rate_line(net_cents: i64) -> FiscalLine {
    let tax_cents = net_cents * 19 / 100;
    FiscalLine {
        line_id: Uuid::new_v4(),
        sku: "SKU-1".into(),
        description: "Coffee beans 1kg".into(),
        quantity: 1,
        unit_price_cents: net_cents + tax_cents,
        net_cents,
        tax_cents,
        gross_cents: net_cents + tax_cents,
        discount_cents: 0,
        tax_rate_bps: 1900,
        tax_category: "standard".into(),
        tax_inclusive: false,
    }
}

fn reduced_rate_line(net_cents: i64) -> FiscalLine {
    let tax_cents = net_cents * 7 / 100;
    FiscalLine {
        line_id: Uuid::new_v4(),
        sku: "SKU-2".into(),
        description: "Bread".into(),
        quantity: 1,
        unit_price_cents: net_cents + tax_cents,
        net_cents,
        tax_cents,
        gross_cents: net_cents + tax_cents,
        discount_cents: 0,
        tax_rate_bps: 700,
        tax_category: "reduced".into(),
        tax_inclusive: false,
    }
}

fn cash_tender(amount_cents: i64) -> FiscalTender {
    FiscalTender {
        tender_id: Uuid::new_v4(),
        tender_type: FiscalTenderType::Cash,
        amount_cents,
        tip_cents: 0,
        provider: None,
        provider_reference: None,
    }
}

fn cash_point() -> CashPoint {
    CashPoint {
        store_id: Uuid::new_v4(),
        register_id: Uuid::new_v4(),
        shift_id: Some(Uuid::new_v4()),
        operator_id: Some("cashier-7".into()),
    }
}

/// A well-formed single-line cash sale, the baseline every test mutates.
fn sale(lines: Vec<FiscalLine>) -> FiscalTransaction {
    let net_cents = lines.iter().map(|l| l.net_cents).sum();
    let tax_cents = lines.iter().map(|l| l.tax_cents).sum();
    let gross_cents = lines.iter().map(|l| l.gross_cents).sum();
    FiscalTransaction {
        transaction_id: Uuid::new_v4(),
        kind: FiscalTransactionKind::Sale,
        reference_transaction_id: None,
        cash_point: cash_point(),
        occurred_at: Utc.with_ymd_and_hms(2026, 3, 4, 10, 30, 0).unwrap(),
        currency: "EUR".into(),
        lines,
        tenders: vec![cash_tender(gross_cents)],
        net_cents,
        tax_cents,
        gross_cents,
    }
}

// ---------- what a fiscal transaction has to reconcile ----------

#[test]
fn a_sale_whose_lines_tenders_and_totals_agree_is_valid() {
    let transaction = sale(vec![standard_rate_line(1000), reduced_rate_line(500)]);

    transaction
        .validate()
        .expect("a reconciling sale should be valid");
}

#[test]
fn a_transaction_with_no_lines_is_refused() {
    let transaction = sale(vec![]);

    let err = transaction
        .validate()
        .expect_err("nothing was sold, so there is nothing to fiscalise");

    assert!(matches!(err, FiscalError::InvalidTransaction { .. }));
}

#[test]
fn a_line_whose_gross_is_not_net_plus_tax_is_refused() {
    let mut transaction = sale(vec![standard_rate_line(1000)]);
    transaction.lines[0].gross_cents += 1;

    let err = transaction
        .validate()
        .expect_err("a line that does not add up must not be signed");

    assert!(matches!(err, FiscalError::InvalidTransaction { .. }));
}

#[test]
fn totals_that_disagree_with_the_lines_are_refused() {
    let mut transaction = sale(vec![standard_rate_line(1000)]);
    transaction.gross_cents += 100;

    let err = transaction
        .validate()
        .expect_err("header totals must be derivable from the lines");

    assert!(matches!(err, FiscalError::InvalidTransaction { .. }));
}

#[test]
fn tenders_that_do_not_cover_the_gross_total_are_refused() {
    let mut transaction = sale(vec![standard_rate_line(1000)]);
    transaction.tenders = vec![cash_tender(1)];

    let err = transaction
        .validate()
        .expect_err("the tender breakdown must account for the whole sale");

    assert!(matches!(err, FiscalError::InvalidTransaction { .. }));
}

#[test]
fn a_tip_is_not_counted_towards_the_fiscal_total() {
    let mut transaction = sale(vec![standard_rate_line(1000)]);
    transaction.tenders[0].tip_cents = 200;

    transaction
        .validate()
        .expect("a tip rides alongside the tender without changing the sale");
}

#[test]
fn a_line_with_no_quantity_is_refused() {
    let mut transaction = sale(vec![standard_rate_line(1000)]);
    transaction.lines[0].quantity = 0;

    let err = transaction
        .validate()
        .expect_err("a sold line always has a quantity");

    assert!(matches!(err, FiscalError::InvalidTransaction { .. }));
}

#[test]
fn a_currency_that_is_not_a_three_letter_code_is_refused() {
    let mut transaction = sale(vec![standard_rate_line(1000)]);
    transaction.currency = "Euro".into();

    let err = transaction
        .validate()
        .expect_err("fiscal exports carry an ISO 4217 code");

    assert!(matches!(err, FiscalError::InvalidTransaction { .. }));
}

#[test]
fn a_tax_rate_above_one_hundred_percent_is_refused() {
    let mut transaction = sale(vec![standard_rate_line(1000)]);
    transaction.lines[0].tax_rate_bps = 10_001;

    let err = transaction
        .validate()
        .expect_err("no jurisdiction taxes above 100%");

    assert!(matches!(err, FiscalError::InvalidTransaction { .. }));
}

#[test]
fn a_sale_must_be_positive_and_a_refund_must_be_negative() {
    let mut refund_shaped_sale = sale(vec![standard_rate_line(1000)]);
    refund_shaped_sale.kind = FiscalTransactionKind::Refund;

    let err = refund_shaped_sale
        .validate()
        .expect_err("a refund that adds money is not a refund");

    assert!(matches!(err, FiscalError::InvalidTransaction { .. }));
}

#[test]
fn a_refund_mirrors_the_sale_with_negative_amounts() {
    let original_order = Uuid::new_v4();
    let mut line = standard_rate_line(1000);
    line.net_cents = -line.net_cents;
    line.tax_cents = -line.tax_cents;
    line.gross_cents = -line.gross_cents;
    let gross = line.gross_cents;

    let refund = FiscalTransaction {
        transaction_id: Uuid::new_v4(),
        kind: FiscalTransactionKind::Refund,
        reference_transaction_id: Some(original_order),
        cash_point: cash_point(),
        occurred_at: Utc.with_ymd_and_hms(2026, 3, 5, 9, 0, 0).unwrap(),
        currency: "EUR".into(),
        net_cents: line.net_cents,
        tax_cents: line.tax_cents,
        gross_cents: gross,
        lines: vec![line],
        tenders: vec![cash_tender(gross)],
    };

    refund.validate().expect("a mirrored refund is valid");
    assert_eq!(refund.reference_transaction_id, Some(original_order));
}

#[test]
fn a_blind_refund_without_an_original_receipt_is_still_valid() {
    let mut line = standard_rate_line(1000);
    line.net_cents = -line.net_cents;
    line.tax_cents = -line.tax_cents;
    line.gross_cents = -line.gross_cents;
    let gross = line.gross_cents;

    let refund = FiscalTransaction {
        transaction_id: Uuid::new_v4(),
        kind: FiscalTransactionKind::Refund,
        reference_transaction_id: None,
        cash_point: cash_point(),
        occurred_at: Utc.with_ymd_and_hms(2026, 3, 5, 9, 0, 0).unwrap(),
        currency: "EUR".into(),
        net_cents: line.net_cents,
        tax_cents: line.tax_cents,
        gross_cents: gross,
        lines: vec![line],
        tenders: vec![cash_tender(gross)],
    };

    refund
        .validate()
        .expect("the till still sold nothing it cannot account for");
}

// ---------- the tax breakdown fiscal exports are built from ----------

#[test]
fn the_tax_breakdown_groups_lines_by_rate() {
    let transaction = sale(vec![
        standard_rate_line(1000),
        standard_rate_line(2000),
        reduced_rate_line(500),
    ]);

    let breakdown = transaction.tax_breakdown();

    assert_eq!(breakdown.len(), 2, "two distinct rates were sold");
    let standard = breakdown
        .iter()
        .find(|b| b.rate_bps == 1900)
        .expect("standard rate group");
    assert_eq!(standard.net_cents, 3000);
    assert_eq!(standard.tax_cents, 570);
    assert_eq!(standard.gross_cents, 3570);

    let reduced = breakdown
        .iter()
        .find(|b| b.rate_bps == 700)
        .expect("reduced rate group");
    assert_eq!(reduced.net_cents, 500);
    assert_eq!(reduced.tax_cents, 35);
}

#[test]
fn the_tax_breakdown_is_ordered_by_rate_so_exports_are_deterministic() {
    let transaction = sale(vec![reduced_rate_line(500), standard_rate_line(1000)]);

    let rates: Vec<u32> = transaction
        .tax_breakdown()
        .iter()
        .map(|b| b.rate_bps)
        .collect();

    assert_eq!(rates, vec![700, 1900]);
}

// ---------- provider contract ----------

#[tokio::test]
async fn the_noop_provider_signs_a_valid_sale_without_a_signature() {
    let provider = NoOpFiscalProvider;
    let transaction = sale(vec![standard_rate_line(1000)]);

    let signature = provider.sign(&transaction).await.expect("noop signs");

    assert_eq!(signature.provider, "noop");
    assert!(signature.fiscal_id.is_none());
    assert!(signature.signature.is_none());
    assert!(
        signature.signed_at >= transaction.occurred_at,
        "the signature is stamped when it was produced"
    );
}

#[tokio::test]
async fn a_provider_refuses_a_transaction_that_does_not_reconcile() {
    let provider = NoOpFiscalProvider;
    let mut transaction = sale(vec![standard_rate_line(1000)]);
    transaction.gross_cents += 5;

    let err = provider
        .sign(&transaction)
        .await
        .expect_err("validation is part of the provider contract");

    assert!(matches!(err, FiscalError::InvalidTransaction { .. }));
}

#[tokio::test]
async fn the_de_tse_provider_signs_a_sale_with_a_fiscal_id_signature_and_qr_payload() {
    let provider = DeTseFiscalProvider::new(true);
    let transaction = sale(vec![standard_rate_line(1000)]);

    let signature = provider.sign(&transaction).await.expect("configured tse");

    assert_eq!(signature.provider, "de_tse");
    assert!(signature.fiscal_id.is_some());
    assert!(signature.signature.is_some());
    assert!(
        signature
            .qr_payload
            .as_deref()
            .is_some_and(|qr| qr.contains(&transaction.transaction_id.to_string())),
        "the receipt QR payload identifies the transaction it belongs to"
    );
}

#[tokio::test]
async fn the_de_tse_provider_also_signs_refunds() {
    let provider = DeTseFiscalProvider::new(true);
    let mut line = standard_rate_line(1000);
    line.net_cents = -line.net_cents;
    line.tax_cents = -line.tax_cents;
    line.gross_cents = -line.gross_cents;
    let gross = line.gross_cents;
    let refund = FiscalTransaction {
        transaction_id: Uuid::new_v4(),
        kind: FiscalTransactionKind::Refund,
        reference_transaction_id: Some(Uuid::new_v4()),
        cash_point: cash_point(),
        occurred_at: Utc.with_ymd_and_hms(2026, 3, 5, 9, 0, 0).unwrap(),
        currency: "EUR".into(),
        net_cents: line.net_cents,
        tax_cents: line.tax_cents,
        gross_cents: gross,
        lines: vec![line],
        tenders: vec![cash_tender(gross)],
    };

    let signature = provider.sign(&refund).await.expect("refunds are signable");

    assert!(signature.fiscal_id.is_some());
}

#[tokio::test]
async fn an_unconfigured_regulated_provider_fails_permanently_not_transiently() {
    let provider = DeTseFiscalProvider::new(false);
    let transaction = sale(vec![standard_rate_line(1000)]);

    let err = provider
        .sign(&transaction)
        .await
        .expect_err("an unconfigured TSE cannot sign");

    assert_eq!(
        err,
        FiscalError::NotConfigured {
            provider: "de_tse".into()
        }
    );
    assert!(
        !err.is_transient(),
        "misconfiguration will not fix itself, so queueing it is pointless"
    );
}

#[test]
fn only_an_unavailable_provider_is_worth_retrying() {
    let unavailable = FiscalError::Unavailable {
        provider: "fiskaly".into(),
        detail: "connection refused".into(),
    };
    let rejected = FiscalError::Rejected {
        provider: "fiskaly".into(),
        code: "invalid_vat_rate".into(),
        detail: "rate not permitted".into(),
    };

    assert!(unavailable.is_transient());
    assert!(!rejected.is_transient());
    assert!(!FiscalError::InvalidTransaction {
        reason: "no lines".into()
    }
    .is_transient());
}

#[test]
fn a_provider_declares_whether_the_till_may_keep_selling_while_it_is_down() {
    assert_eq!(
        NoOpFiscalProvider.offline_policy(),
        OfflinePolicy::SignLater
    );
    assert_eq!(
        DeTseFiscalProvider::new(true).offline_policy(),
        OfflinePolicy::SignLater,
        "German law lets the till keep selling and sign later after a TSE outage"
    );
}

#[test]
fn a_transaction_survives_a_round_trip_through_the_sign_later_queue() {
    let transaction = sale(vec![standard_rate_line(1000), reduced_rate_line(500)]);

    let json = serde_json::to_string(&transaction).expect("serialise");
    let restored: FiscalTransaction = serde_json::from_str(&json).expect("deserialise");

    assert_eq!(restored, transaction);
}
