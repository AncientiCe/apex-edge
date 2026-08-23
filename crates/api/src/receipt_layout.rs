//! Turns a stored receipt payload into a printable receipt.
//!
//! The same payload JSON already drives the PDF template, so a printed receipt and a
//! fetched PDF describe the same sale. Nothing here reads the database: given a payload
//! it is a pure function, which is why it can be pinned in tests.

use apex_edge_adapters_hardware::{ReceiptDocument, TextStyle};
use serde_json::Value;

/// Builds a receipt for `paper_width` character columns.
pub fn receipt_document(payload: &Value, paper_width: usize) -> ReceiptDocument {
    let mut document = ReceiptDocument::new(paper_width);

    document = document.text(store_name(payload), TextStyle::centred().bold().double());
    if let Some(address) = non_empty(payload, "store_address") {
        document = document.text(address, TextStyle::centred());
    }

    document = document.divider();
    if let Some(order_id) = payload.get("order_id").and_then(Value::as_str) {
        document = document.text(
            format!("Order {}", short_id(order_id)),
            TextStyle::default(),
        );
    }
    if let Some(created_at) = non_empty(payload, "created_at") {
        document = document.text(readable_timestamp(&created_at), TextStyle::default());
    }
    if let Some(customer) = non_empty(payload, "customer_name") {
        document = document.text(format!("Customer: {customer}"), TextStyle::default());
    }
    document = document.divider();

    for line in array(payload, "lines") {
        let name = line
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Item")
            .to_string();
        let quantity = line.get("quantity").and_then(Value::as_i64).unwrap_or(1);
        let label = if quantity == 1 {
            name
        } else {
            format!("{quantity} x {name}")
        };
        document = document.columns(label, money(cents(line, "line_total_cents")));
        // A discount the customer cannot see on the receipt is a discount they will ask
        // about at the counter.
        let discount = cents(line, "discount_cents");
        if discount > 0 {
            document = document.columns("  Discount", format!("-{}", money(discount)));
        }
    }

    document = document.divider();
    document = document.columns("Subtotal", money(cents(payload, "subtotal_cents")));
    let discount = cents(payload, "discount_cents");
    if discount > 0 {
        document = document.columns("Discount", format!("-{}", money(discount)));
    }
    document = document.columns("Tax", money(cents(payload, "tax_cents")));
    document = document.columns("TOTAL", money(cents(payload, "total_cents")));
    document = document.divider();

    for payment in array(payload, "payments") {
        document = document.columns(tender_label(payment), money(cents(payment, "amount_cents")));
        let tip = cents(payment, "tip_amount_cents");
        if tip > 0 {
            document = document.columns("  Tip", money(tip));
        }
    }

    document = document.text("", TextStyle::default());
    document = document.text("Thank you", TextStyle::centred());
    if let Some(order_id) = payload.get("order_id").and_then(Value::as_str) {
        // Machine-readable order reference: this is what a returns desk scans, and what
        // fiscal jurisdictions later replace with a signature payload.
        document = document.qr(format!("apex-edge:order:{order_id}"));
    }

    document.feed(3).cut()
}

fn store_name(payload: &Value) -> String {
    non_empty(payload, "store_name").unwrap_or_else(|| "RECEIPT".to_string())
}

fn non_empty(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn array<'a>(payload: &'a Value, key: &str) -> &'a [Value] {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn cents(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// Cents to a decimal amount. Deliberately unit-agnostic: the store's currency is on the
/// template, and printing a symbol the code page cannot represent is worse than none.
fn money(cents: i64) -> String {
    let negative = cents < 0;
    let cents = cents.abs();
    let formatted = format!("{}.{:02}", cents / 100, cents % 100);
    if negative {
        format!("-{formatted}")
    } else {
        formatted
    }
}

/// A tender label an operator recognises, preferring what actually took the money.
fn tender_label(payment: &Value) -> String {
    let provider = payment.get("provider").and_then(Value::as_str);
    let tender_type = payment.get("tender_type").and_then(Value::as_str);
    match (tender_type, provider) {
        (Some(tender), _) if !tender.is_empty() && tender != "unknown" => tender.to_uppercase(),
        (_, Some(provider)) if !provider.is_empty() => provider.to_uppercase(),
        _ => "PAYMENT".to_string(),
    }
}

fn short_id(id: &str) -> &str {
    // Eight characters is enough to find an order and short enough to read aloud.
    id.get(..8).unwrap_or(id)
}

/// RFC 3339 down to whole seconds, with the `T` removed. Nobody wants to read
/// `2026-08-20T16:18:34.440309300Z` off a receipt.
fn readable_timestamp(raw: &str) -> String {
    let trimmed = raw.split('.').next().unwrap_or(raw);
    let trimmed = trimmed.trim_end_matches('Z');
    trimmed.replacen('T', " ", 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_edge_adapters_hardware::{codepage, EscPosEncoder, ReceiptEncoder};

    fn payload() -> Value {
        serde_json::json!({
            "order_id": "0191f0c4-1234-7000-8000-abcdefabcdef",
            "created_at": "2026-08-20T16:18:34.440309300Z",
            "store_name": "Apex Coffee",
            "store_address": "12 Market Street",
            "customer_name": "",
            "subtotal_cents": 2_500,
            "discount_cents": 250,
            "tax_cents": 225,
            "total_cents": 2_475,
            "lines": [
                {"name": "Filter Coffee", "quantity": 2, "line_total_cents": 2_500, "discount_cents": 250},
            ],
            "payments": [
                {"tender_type": "cash", "amount_cents": 2_475, "tip_amount_cents": 100},
            ],
        })
    }

    fn printed(payload: &Value, width: usize) -> String {
        let document = receipt_document(payload, width);
        codepage::decode(&EscPosEncoder.encode(&document).expect("encode"))
    }

    #[test]
    fn a_receipt_reads_like_a_receipt() {
        let text = printed(&payload(), 32);

        for expected in [
            "Apex Coffee",
            "12 Market Street",
            "Order 0191f0c4",
            "2026-08-20 16:18:34",
            "2 x Filter Coffee",
            "25.00",
            "Discount",
            "-2.50",
            "Tax",
            "2.25",
            "TOTAL",
            "24.75",
            "CASH",
            "Tip",
            "1.00",
            "Thank you",
        ] {
            assert!(text.contains(expected), "missing {expected} in:\n{text}");
        }
    }

    #[test]
    fn every_printed_line_fits_the_paper() {
        // A line longer than the paper is silently clipped by the printer, and it is
        // always the price that falls off the end.
        for width in [32, 42, 48] {
            // Codes are printed as graphics and sized by the printer, so their text
            // stand-in has no paper width to exceed.
            for line in receipt_document(&payload(), width)
                .plain_text()
                .lines()
                .filter(|line| !line.starts_with('['))
            {
                assert!(
                    line.chars().count() <= width,
                    "{width} column paper got a {} column line: {line:?}",
                    line.chars().count()
                );
            }
        }
    }

    #[test]
    fn a_payload_missing_everything_still_produces_printable_paper() {
        // Receipt payloads come from templates and callers; a thin one must degrade, not
        // panic, and must still cut the paper so the next receipt is not attached to it.
        let text = printed(&serde_json::json!({}), 32);

        assert!(text.contains("RECEIPT"));
        assert!(text.contains("TOTAL"));
        assert!(text.contains("0.00"));
    }

    #[test]
    fn a_zero_discount_is_left_off_rather_than_printed_as_nothing() {
        let text = printed(
            &serde_json::json!({"total_cents": 1_000, "lines": [], "payments": []}),
            32,
        );

        assert!(!text.contains("Discount"), "got:\n{text}");
    }

    #[test]
    fn the_order_reference_is_machine_readable() {
        let document = receipt_document(&payload(), 32);
        let bytes = EscPosEncoder.encode(&document).expect("encode");

        assert!(
            codepage::decode(&bytes)
                .contains("apex-edge:order:0191f0c4-1234-7000-8000-abcdefabcdef"),
            "the QR payload must identify the order"
        );
    }

    #[test]
    fn a_provider_name_is_used_when_the_tender_type_is_unknown() {
        let text = printed(
            &serde_json::json!({
                "total_cents": 1_000,
                "payments": [{"tender_type": "unknown", "provider": "stripe_terminal", "amount_cents": 1_000}],
            }),
            42,
        );

        assert!(text.contains("STRIPE_TERMINAL"), "got:\n{text}");
    }
}
