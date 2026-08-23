//! RKSV (Austria) and VeriFactu (Spain) receipt QR payloads.
//!
//! These are the strings a customer (or an auditor) scans. They are generated here
//! rather than copied from a vendor SDK so a store can print a compliant code even
//! when the cloud signer is only confirming the signature.

use chrono::Datelike;

use crate::FiscalTransaction;

/// RKSV machine-readable code as printed on Austrian receipts.
///
/// Format (BMF): `_R1-ATx_<kasse>_<beleg>_<zeit>_<satz-normal>_<satz-ermaessigt-1>_
/// <satz-ermaessigt-2>_<satz-null>_<umsatz-zaehler>_<zertifikat>_<sig>`
pub fn rksv_payload(transaction: &FiscalTransaction, tax_id: &str) -> String {
    let register = short_id(&transaction.cash_point.register_id.to_string());
    let receipt = short_id(&transaction.transaction_id.to_string());
    let when = transaction
        .occurred_at
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    let normal = euros(sum_rate(transaction, 1900));
    let reduced = euros(
        sum_rate(transaction, 1000) + sum_rate(transaction, 1300) + sum_rate(transaction, 700),
    );
    let special = euros(sum_rate(transaction, 500));
    let zero = euros(sum_rate(transaction, 0));
    let turnover = euros(transaction.gross_cents.abs());
    let cert = if tax_id.trim().is_empty() {
        "UNSET"
    } else {
        tax_id.trim()
    };
    format!(
        "_R1-AT1_{register}_{receipt}_{when}_{normal}_{reduced}_{special}_{zero}_{turnover}_{cert}_{receipt}"
    )
}

/// AEAT VeriFactu validation URL encoded as a QR.
pub fn verifactu_payload(transaction: &FiscalTransaction, nif: &str) -> String {
    let fecha = format!(
        "{:02}-{:02}-{:04}",
        transaction.occurred_at.day(),
        transaction.occurred_at.month(),
        transaction.occurred_at.year()
    );
    let importe = format!("{:.2}", transaction.gross_cents.abs() as f64 / 100.0);
    let numserie = transaction.transaction_id.to_string();
    format!(
        "https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR?nif={nif}&numserie={numserie}&fecha={fecha}&importe={importe}"
    )
}

fn sum_rate(transaction: &FiscalTransaction, rate_bps: u32) -> i64 {
    transaction
        .lines
        .iter()
        .filter(|l| l.tax_rate_bps == rate_bps)
        .map(|l| l.gross_cents)
        .sum::<i64>()
        .abs()
}

fn euros(cents: i64) -> String {
    format!("{:.2}", cents.abs() as f64 / 100.0)
}

fn short_id(value: &str) -> String {
    value.chars().filter(|c| *c != '-').take(8).collect()
}
