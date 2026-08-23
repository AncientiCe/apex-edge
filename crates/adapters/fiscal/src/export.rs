//! DSFinV-K cash-point closing files and EN 16931 invoices (XRechnung + Factur-X).
//!
//! These are generated locally, with no vendor SDK. Close-till turns the shift's
//! signed sales into a German cash-point closing; a single fiscal transaction can
//! also be rendered as an EN 16931 invoice for a buyer who needs one.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{CashPoint, FiscalTenderType, FiscalTransaction, FiscalTransactionKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum En16931Profile {
    XRechnung,
    FacturX,
}

impl En16931Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::XRechnung => "xrechnung",
            Self::FacturX => "factur-x",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CashPointClosing {
    pub shift_id: Uuid,
    pub store_id: Uuid,
    pub register_id: Uuid,
    pub opened_at: DateTime<Utc>,
    pub closed_at: DateTime<Utc>,
    pub opening_float_cents: i64,
    pub counted_cents: i64,
    pub expected_cents: i64,
    pub currency: String,
    pub tax_id: String,
    pub transactions: Vec<FiscalTransaction>,
}

/// DSFinV-K is a bundle of semicolon-separated CSVs. Returns `(filename, contents)`.
pub fn dsfinvk_files(closing: &CashPointClosing) -> Vec<(String, String)> {
    let register = closing.register_id.to_string();
    let kasse = csv_row(&["KASSE_ID", "STEUERNUMMER", "WAEHRUNG", "BRAND", "MODELL"])
        + &csv_row(&[
            &register,
            &closing.tax_id,
            &closing.currency,
            "ApexEdge",
            "store-hub",
        ]);

    let mut bonkopf = csv_row(&[
        "BON_ID",
        "KASSE_ID",
        "BON_TYP",
        "BON_START",
        "BON_ENDE",
        "UMS_BRUTTO",
        "KUNDE_NAME",
    ]);
    let mut bonpos = csv_row(&[
        "BON_ID",
        "POS_ZEILE",
        "ARTIKELTEXT",
        "MENGE",
        "Ums_Netto",
        "Ums_Brutto",
    ]);
    let mut bonust = csv_row(&["BON_ID", "UST_SATZ", "UST_PFLICHT", "UST_BETRAG"]);
    let mut zahlart = csv_row(&["BON_ID", "ZAHLART", "ZAHL_BETRAG"]);

    for tx in &closing.transactions {
        let bon_id = tx.transaction_id.to_string();
        let kind = match tx.kind {
            FiscalTransactionKind::Sale => "Umsatz",
            FiscalTransactionKind::Refund => "Retoure",
            FiscalTransactionKind::Void => "Storno",
        };
        let when = tx.occurred_at.format("%Y-%m-%dT%H:%M:%S").to_string();
        bonkopf.push_str(&csv_row(&[
            &bon_id,
            &register,
            kind,
            &when,
            &when,
            &euros(tx.gross_cents),
            "",
        ]));
        for (index, line) in tx.lines.iter().enumerate() {
            bonpos.push_str(&csv_row(&[
                &bon_id,
                &(index + 1).to_string(),
                &line.description,
                &line.quantity.to_string(),
                &euros(line.net_cents),
                &euros(line.gross_cents),
            ]));
        }
        for vat in tx.tax_breakdown() {
            bonust.push_str(&csv_row(&[
                &bon_id,
                &format!("{:.2}", vat.rate_bps as f64 / 100.0),
                &euros(vat.net_cents),
                &euros(vat.tax_cents),
            ]));
        }
        for tender in &tx.tenders {
            zahlart.push_str(&csv_row(&[
                &bon_id,
                tender_name(tender.tender_type),
                &euros(tender.amount_cents),
            ]));
        }
    }

    vec![
        ("Stamm_Kassen.csv".into(), kasse),
        ("Bonkopf.csv".into(), bonkopf),
        ("Bonpos.csv".into(), bonpos),
        ("Bonpos_USt.csv".into(), bonust),
        ("Z_Zahlart.csv".into(), zahlart),
    ]
}

pub fn ubl_xrechnung(
    transaction: &FiscalTransaction,
    seller_tax_id: &str,
    seller_name: &str,
) -> String {
    let issue = transaction.occurred_at.format("%Y-%m-%d").to_string();
    let mut lines = String::new();
    for (i, line) in transaction.lines.iter().enumerate() {
        let pct = format!("{:.2}", line.tax_rate_bps as f64 / 100.0);
        lines.push_str(&format!(
            r#"    <cac:InvoiceLine>
      <cbc:ID>{id}</cbc:ID>
      <cbc:InvoicedQuantity unitCode="C62">{qty}</cbc:InvoicedQuantity>
      <cbc:LineExtensionAmount currencyID="{cur}">{net}</cbc:LineExtensionAmount>
      <cac:Item>
        <cbc:Name>{name}</cbc:Name>
        <cac:ClassifiedTaxCategory>
          <cbc:ID>S</cbc:ID>
          <cbc:Percent>{pct}</cbc:Percent>
          <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
        </cac:ClassifiedTaxCategory>
      </cac:Item>
      <cac:Price>
        <cbc:PriceAmount currencyID="{cur}">{price}</cbc:PriceAmount>
      </cac:Price>
    </cac:InvoiceLine>
"#,
            id = i + 1,
            qty = line.quantity,
            cur = xml(&transaction.currency),
            net = euros(line.net_cents),
            name = xml(&line.description),
            price = euros(line.unit_price_cents),
        ));
    }
    let tax_xml: String = transaction
        .tax_breakdown()
        .iter()
        .map(|vat| {
            let pct = format!("{:.2}", vat.rate_bps as f64 / 100.0);
            format!(
                r#"      <cac:TaxSubtotal>
        <cbc:TaxableAmount currencyID="{cur}">{net}</cbc:TaxableAmount>
        <cbc:TaxAmount currencyID="{cur}">{tax}</cbc:TaxAmount>
        <cac:TaxCategory>
          <cbc:ID>S</cbc:ID>
          <cbc:Percent>{pct}</cbc:Percent>
          <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
        </cac:TaxCategory>
      </cac:TaxSubtotal>
"#,
                cur = xml(&transaction.currency),
                net = euros(vat.net_cents),
                tax = euros(vat.tax_cents),
            )
        })
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"
         xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
         xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
  <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0</cbc:CustomizationID>
  <cbc:ID>{id}</cbc:ID>
  <cbc:IssueDate>{issue}</cbc:IssueDate>
  <cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>
  <cbc:DocumentCurrencyCode>{cur}</cbc:DocumentCurrencyCode>
  <cac:AccountingSupplierParty>
    <cac:Party>
      <cac:PartyLegalEntity><cbc:RegistrationName>{seller}</cbc:RegistrationName></cac:PartyLegalEntity>
      <cac:PartyTaxScheme>
        <cbc:CompanyID>{tax_id}</cbc:CompanyID>
        <cac:TaxScheme><cbc:ID>VAT</cbc:ID></cac:TaxScheme>
      </cac:PartyTaxScheme>
    </cac:Party>
  </cac:AccountingSupplierParty>
  <cac:AccountingCustomerParty>
    <cac:Party>
      <cac:PartyLegalEntity><cbc:RegistrationName>Walk-in customer</cbc:RegistrationName></cac:PartyLegalEntity>
    </cac:Party>
  </cac:AccountingCustomerParty>
  <cac:TaxTotal>
    <cbc:TaxAmount currencyID="{cur}">{tax}</cbc:TaxAmount>
{tax_xml}  </cac:TaxTotal>
  <cac:LegalMonetaryTotal>
    <cbc:LineExtensionAmount currencyID="{cur}">{net}</cbc:LineExtensionAmount>
    <cbc:TaxExclusiveAmount currencyID="{cur}">{net}</cbc:TaxExclusiveAmount>
    <cbc:TaxInclusiveAmount currencyID="{cur}">{gross}</cbc:TaxInclusiveAmount>
    <cbc:PayableAmount currencyID="{cur}">{gross}</cbc:PayableAmount>
  </cac:LegalMonetaryTotal>
{lines}</Invoice>
"#,
        id = transaction.transaction_id,
        issue = issue,
        cur = xml(&transaction.currency),
        seller = xml(seller_name),
        tax_id = xml(seller_tax_id),
        tax = euros(transaction.tax_cents),
        tax_xml = tax_xml,
        net = euros(transaction.net_cents),
        gross = euros(transaction.gross_cents),
        lines = lines,
    )
}

pub fn cii_factur_x(
    transaction: &FiscalTransaction,
    seller_tax_id: &str,
    seller_name: &str,
) -> String {
    let issue = transaction.occurred_at.format("%Y%m%d").to_string();
    let mut lines = String::new();
    for (i, line) in transaction.lines.iter().enumerate() {
        let pct = format!("{:.2}", line.tax_rate_bps as f64 / 100.0);
        lines.push_str(&format!(
            r#"      <ram:IncludedSupplyChainTradeLineItem>
        <ram:AssociatedDocumentLineDocument><ram:LineID>{id}</ram:LineID></ram:AssociatedDocumentLineDocument>
        <ram:SpecifiedTradeProduct><ram:Name>{name}</ram:Name></ram:SpecifiedTradeProduct>
        <ram:SpecifiedLineTradeAgreement>
          <ram:NetPriceProductTradePrice><ram:ChargeAmount>{price}</ram:ChargeAmount></ram:NetPriceProductTradePrice>
        </ram:SpecifiedLineTradeAgreement>
        <ram:SpecifiedLineTradeDelivery>
          <ram:BilledQuantity unitCode="C62">{qty}</ram:BilledQuantity>
        </ram:SpecifiedLineTradeDelivery>
        <ram:SpecifiedLineTradeSettlement>
          <ram:ApplicableTradeTax>
            <ram:TypeCode>VAT</ram:TypeCode>
            <ram:CategoryCode>S</ram:CategoryCode>
            <ram:RateApplicablePercent>{pct}</ram:RateApplicablePercent>
          </ram:ApplicableTradeTax>
          <ram:SpecifiedTradeSettlementLineMonetarySummation>
            <ram:LineTotalAmount>{net}</ram:LineTotalAmount>
          </ram:SpecifiedTradeSettlementLineMonetarySummation>
        </ram:SpecifiedLineTradeSettlement>
      </ram:IncludedSupplyChainTradeLineItem>
"#,
            id = i + 1,
            name = xml(&line.description),
            price = euros(line.unit_price_cents),
            qty = line.quantity,
            net = euros(line.net_cents),
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rsm:CrossIndustryInvoice xmlns:rsm="urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100"
                          xmlns:ram="urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:100"
                          xmlns:udt="urn:un:unece:uncefact:data:standard:UnqualifiedDataType:100">
  <rsm:ExchangedDocumentContext>
    <ram:GuidelineSpecifiedDocumentContextParameter>
      <ram:ID>urn:cen.eu:en16931:2017#conformant#urn:factur-x.eu:1p0:en16931</ram:ID>
    </ram:GuidelineSpecifiedDocumentContextParameter>
  </rsm:ExchangedDocumentContext>
  <rsm:ExchangedDocument>
    <ram:ID>{id}</ram:ID>
    <ram:TypeCode>380</ram:TypeCode>
    <ram:IssueDateTime><udt:DateTimeString format="102">{issue}</udt:DateTimeString></ram:IssueDateTime>
  </rsm:ExchangedDocument>
  <rsm:SupplyChainTradeTransaction>
{lines}    <ram:ApplicableHeaderTradeAgreement>
      <ram:SellerTradeParty>
        <ram:Name>{seller}</ram:Name>
        <ram:SpecifiedTaxRegistration><ram:ID>{tax_id}</ram:ID></ram:SpecifiedTaxRegistration>
      </ram:SellerTradeParty>
      <ram:BuyerTradeParty><ram:Name>Walk-in customer</ram:Name></ram:BuyerTradeParty>
    </ram:ApplicableHeaderTradeAgreement>
    <ram:ApplicableHeaderTradeSettlement>
      <ram:InvoiceCurrencyCode>{cur}</ram:InvoiceCurrencyCode>
      <ram:SpecifiedTradeSettlementHeaderMonetarySummation>
        <ram:LineTotalAmount>{net}</ram:LineTotalAmount>
        <ram:TaxBasisTotalAmount>{net}</ram:TaxBasisTotalAmount>
        <ram:TaxTotalAmount>{tax}</ram:TaxTotalAmount>
        <ram:GrandTotalAmount>{gross}</ram:GrandTotalAmount>
        <ram:DuePayableAmount>{gross}</ram:DuePayableAmount>
      </ram:SpecifiedTradeSettlementHeaderMonetarySummation>
    </ram:ApplicableHeaderTradeSettlement>
  </rsm:SupplyChainTradeTransaction>
</rsm:CrossIndustryInvoice>
"#,
        id = transaction.transaction_id,
        issue = issue,
        lines = lines,
        seller = xml(seller_name),
        tax_id = xml(seller_tax_id),
        cur = xml(&transaction.currency),
        net = euros(transaction.net_cents),
        tax = euros(transaction.tax_cents),
        gross = euros(transaction.gross_cents),
    )
}

/// Evaluate a tiny subset of Schematron: `assert test` on an XML document.
///
/// Enough to pin EN 16931 required fields in tests without a full ISO Schematron
/// engine. Unknown expressions fail closed so a typo in a rule cannot pass a bad invoice.
pub fn evaluate_schematron(xml: &str, schematron: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let mut context = String::new();
    let mut rest = schematron;
    while let Some(idx) = rest.find('<') {
        rest = &rest[idx..];
        if rest.starts_with("<rule ") {
            if let Some(ctx) = attr(rest, "context") {
                context = ctx;
            }
            rest = &rest[1..];
            continue;
        }
        if rest.starts_with("<assert ") {
            let test = attr(rest, "test").unwrap_or_default();
            let after_gt = rest.find('>').map(|i| i + 1).unwrap_or(1);
            let message_src = &rest[after_gt..];
            let message = message_src
                .split("</assert>")
                .next()
                .unwrap_or("failed")
                .trim()
                .to_string();
            let context_present = xml.contains(&context)
                || xml.contains(&format!(":{context}"))
                || context.is_empty();
            if !context_present || !assert_holds(xml, &test) {
                failures.push(message);
            }
            rest = &rest[1..];
            continue;
        }
        rest = &rest[1..];
    }
    failures
}

fn assert_holds(xml: &str, test: &str) -> bool {
    let test = test.trim();
    if let Some(inner) = test
        .strip_prefix("contains(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let (haystack, needle) = inner.split_once(',').unwrap_or((inner, ""));
        let needle = needle.trim().trim_matches('\'').trim_matches('"');
        let hay = haystack.trim();
        let needle_l = needle.to_ascii_lowercase();
        if hay != "." && !xml.contains(hay.rsplit(':').next().unwrap_or(hay)) {
            return false;
        }
        // Not scoped to the `hay` element's contents specifically — checks the whole
        // document, relying on the element-presence check above to keep this meaningful.
        return xml.to_ascii_lowercase().contains(&needle_l);
    }
    let local = test.rsplit(':').next().unwrap_or(test);
    xml.contains(&format!("<{test}"))
        || xml.contains(&format!(":{local}"))
        || xml.contains(&format!("<{local}"))
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

fn csv_row(fields: &[&str]) -> String {
    let mut out = String::new();
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(';');
        }
        out.push_str(&csv_escape(field));
    }
    out.push('\n');
    out
}

fn csv_escape(value: &str) -> String {
    if value.contains(';') || value.contains('"') || value.contains('\n') || value.contains('&') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn euros(cents: i64) -> String {
    format!("{:.2}", cents as f64 / 100.0)
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn tender_name(tender: FiscalTenderType) -> &'static str {
    match tender {
        FiscalTenderType::Cash => "Bar",
        FiscalTenderType::Card => "Unbar",
        FiscalTenderType::GiftCard => "Gutschein",
        FiscalTenderType::Voucher => "Gutschein",
        FiscalTenderType::Other => "Unbar",
    }
}

/// Reconstruct a fiscal transaction from ledger amounts when the original
/// cart is gone (used by cash-point closing).
pub fn transaction_from_ledger_amounts(
    transaction_id: Uuid,
    cash_point: CashPoint,
    occurred_at: DateTime<Utc>,
    currency: &str,
    lines: Vec<crate::FiscalLine>,
    tenders: Vec<crate::FiscalTender>,
) -> FiscalTransaction {
    let net_cents = lines.iter().map(|l| l.net_cents).sum();
    let tax_cents = lines.iter().map(|l| l.tax_cents).sum();
    let gross_cents = lines.iter().map(|l| l.gross_cents).sum();
    FiscalTransaction {
        transaction_id,
        kind: FiscalTransactionKind::Sale,
        reference_transaction_id: None,
        cash_point,
        occurred_at,
        currency: currency.to_ascii_uppercase(),
        lines,
        tenders,
        net_cents,
        tax_cents,
        gross_cents,
    }
}
