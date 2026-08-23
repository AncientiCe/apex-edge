//! Byte-exact encoder tests.
//!
//! A receipt printer has no error channel worth speaking of: send it a byte wrong and
//! it prints garbage, or nothing, and nobody finds out until a customer complains. So
//! the encoders are pinned to golden byte streams that are reviewed as carefully as the
//! code, and every one of these tests runs with no device attached.

use apex_edge_adapters_hardware::{
    EscPosEncoder, HardwareError, ReceiptDocument, ReceiptEncoder, StarLineModeEncoder, Symbology,
    TextStyle,
};

mod golden;

/// The receipt shared by the two golden files: one styled heading, a plain line, a
/// divider, two aligned money columns, a feed and a cut.
fn sample_receipt() -> ReceiptDocument {
    ReceiptDocument::new(32)
        .text("APEX STORE", TextStyle::centred().bold().double())
        .text("Order 1001", TextStyle::centred())
        .divider()
        .columns("Coffee", "3.50")
        .columns("TOTAL", "3.50")
        .feed(2)
        .cut()
}

fn sample_codes() -> ReceiptDocument {
    ReceiptDocument::new(32)
        .qr("https://apex.example/r/1001")
        .barcode(Symbology::Code128, "1001")
}

#[test]
fn escpos_encodes_the_sample_receipt_byte_for_byte() {
    let expected = golden::load("receipt_escpos.hex");
    let actual = EscPosEncoder.encode(&sample_receipt()).expect("encode");

    golden::assert_bytes_eq(&expected, &actual);
}

#[test]
fn star_line_mode_encodes_the_sample_receipt_byte_for_byte() {
    let expected = golden::load("receipt_star.hex");
    let actual = StarLineModeEncoder
        .encode(&sample_receipt())
        .expect("encode");

    golden::assert_bytes_eq(&expected, &actual);
}

#[test]
fn escpos_encodes_qr_and_barcode_byte_for_byte() {
    let expected = golden::load("codes_escpos.hex");
    let actual = EscPosEncoder.encode(&sample_codes()).expect("encode");

    golden::assert_bytes_eq(&expected, &actual);
}

#[test]
fn star_line_mode_encodes_qr_and_barcode_byte_for_byte() {
    let expected = golden::load("codes_star.hex");
    let actual = StarLineModeEncoder.encode(&sample_codes()).expect("encode");

    golden::assert_bytes_eq(&expected, &actual);
}

#[test]
fn each_encoder_kicks_the_drawer_with_its_own_dialect() {
    // Wrong dialect here means the drawer silently never opens, so both are pinned.
    assert_eq!(
        EscPosEncoder.drawer_kick(),
        vec![0x1B, 0x70, 0x00, 0x19, 0xFA],
        "ESC p m t1 t2: pin 2, 50ms on, 500ms off"
    );
    assert_eq!(
        StarLineModeEncoder.drawer_kick(),
        vec![0x1B, 0x07, 0x05, 0x32, 0x07],
        "ESC BEL n1 n2 then BEL to fire drawer 1"
    );
}

#[test]
fn every_job_starts_by_initialising_the_printer() {
    // Without ESC @ a job inherits whatever state the previous job left behind, which
    // is how one bold line turns every later receipt bold.
    assert!(EscPosEncoder
        .encode(&sample_receipt())
        .expect("encode")
        .starts_with(&[0x1B, 0x40]));
    assert!(StarLineModeEncoder
        .encode(&sample_receipt())
        .expect("encode")
        .starts_with(&[0x1B, 0x40]));
}

#[test]
fn columns_are_padded_so_prices_line_up_on_the_right_edge() {
    let doc = ReceiptDocument::new(20).columns("Milk", "1.05");
    let bytes = EscPosEncoder.encode(&doc).expect("encode");

    assert!(
        contains(&bytes, b"Milk            1.05\n"),
        "expected a 20 column row, got {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn a_long_column_label_is_truncated_rather_than_pushing_the_price_off_the_paper() {
    let doc = ReceiptDocument::new(16).columns("Single Origin Guatemala", "12.00");
    let bytes = EscPosEncoder.encode(&doc).expect("encode");

    assert!(
        contains(&bytes, b"Single Ori 12.00\n"),
        "expected the label clipped to make room for the price, got {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn text_wider_than_the_paper_wraps_on_word_boundaries() {
    let doc = ReceiptDocument::new(16).text("The quick brown fox jumps", TextStyle::default());
    let bytes = EscPosEncoder.encode(&doc).expect("encode");

    assert!(
        contains(&bytes, b"The quick brown\nfox jumps\n"),
        "expected wrapped lines, got {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn double_width_text_wraps_at_half_the_column_count() {
    // Double width means half as many characters fit; forgetting that prints a heading
    // that runs off the edge of the paper.
    let doc = ReceiptDocument::new(32).text("APEX STORE DOWNTOWN", TextStyle::default().double());
    let bytes = EscPosEncoder.encode(&doc).expect("encode");

    assert!(
        contains(&bytes, b"APEX STORE\nDOWNTOWN\n"),
        "expected a wrap at 16 columns, got {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn accented_text_is_transcoded_to_the_printer_code_page() {
    // UTF-8 straight down the wire prints mojibake: the printer is on a single byte
    // code page, so "café" must become one byte per character.
    let doc = ReceiptDocument::new(32).text("Café", TextStyle::default());
    let bytes = EscPosEncoder.encode(&doc).expect("encode");

    assert!(
        contains(&bytes, &[b'C', b'a', b'f', 0x82, b'\n']),
        "expected CP437 0x82 for e-acute, got {bytes:02X?}"
    );
}

#[test]
fn a_currency_symbol_missing_from_the_code_page_degrades_to_letters() {
    // CP437 has no euro sign. Printing "?3.50" on a receipt is worse than "EUR3.50".
    let doc = ReceiptDocument::new(32).text("€3.50", TextStyle::default());
    let bytes = EscPosEncoder.encode(&doc).expect("encode");

    assert!(
        contains(&bytes, b"EUR3.50\n"),
        "expected a readable fallback, got {:?}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn an_untranslatable_character_becomes_a_placeholder_not_a_stray_byte() {
    let doc = ReceiptDocument::new(32).text("cost 1000\u{5186}", TextStyle::default());
    let bytes = EscPosEncoder.encode(&doc).expect("encode");

    assert!(
        contains(&bytes, b"cost 1000?\n"),
        "expected a single placeholder byte, got {bytes:02X?}"
    );
}

#[test]
fn an_empty_document_is_refused_rather_than_spitting_out_blank_paper() {
    let empty = ReceiptDocument::new(32);

    assert_eq!(
        EscPosEncoder.encode(&empty),
        Err(HardwareError::EmptyPayload {
            operation: "encode_receipt".into()
        })
    );
    assert_eq!(
        StarLineModeEncoder.encode(&empty),
        Err(HardwareError::EmptyPayload {
            operation: "encode_receipt".into()
        })
    );
}

#[test]
fn a_barcode_with_no_data_is_refused() {
    let doc = ReceiptDocument::new(32).barcode(Symbology::Code128, "");

    assert_eq!(
        EscPosEncoder.encode(&doc),
        Err(HardwareError::EmptyPayload {
            operation: "encode_barcode".into()
        })
    );
}

#[test]
fn a_barcode_the_symbology_cannot_carry_is_refused_at_encode_time() {
    // A printer given an impossible payload prints bars that no scanner reads, and the
    // problem only surfaces at a returns desk. Fail here, where someone is watching.
    let too_short = ReceiptDocument::new(32).barcode(Symbology::Ean13, "12345");
    let not_digits = ReceiptDocument::new(32).barcode(Symbology::Ean13, "12345678901A");

    for doc in [too_short, not_digits] {
        assert!(
            matches!(
                EscPosEncoder.encode(&doc),
                Err(HardwareError::InvalidPayload { .. })
            ),
            "EAN-13 must reject payloads it cannot represent"
        );
    }
}

#[test]
fn a_valid_ean13_reaches_both_dialects_with_the_right_symbology_code() {
    let doc = ReceiptDocument::new(32).barcode(Symbology::Ean13, "4006381333931");

    let escpos = EscPosEncoder.encode(&doc).expect("encode");
    let star = StarLineModeEncoder.encode(&doc).expect("encode");

    assert!(
        contains(&escpos, &[0x1D, 0x6B, 67, 13]),
        "GS k 67 with 13 data bytes, got {escpos:02X?}"
    );
    assert!(
        contains(&star, &[0x1B, 0x62, 3]),
        "ESC b type 3, got {star:02X?}"
    );
}

#[test]
fn non_ascii_barcode_data_is_refused_rather_than_silently_mangled() {
    let doc = ReceiptDocument::new(32).barcode(Symbology::Code128, "café");

    assert!(matches!(
        EscPosEncoder.encode(&doc),
        Err(HardwareError::InvalidPayload { .. })
    ));
}

#[test]
fn a_qr_code_with_no_data_is_refused() {
    let doc = ReceiptDocument::new(32).qr("");

    assert_eq!(
        StarLineModeEncoder.encode(&doc),
        Err(HardwareError::EmptyPayload {
            operation: "encode_qr".into()
        })
    );
}

#[test]
fn encoders_report_the_dialect_they_speak() {
    // The name ends up on the hardware metrics label, so it has to be stable.
    assert_eq!(EscPosEncoder.name(), "escpos");
    assert_eq!(StarLineModeEncoder.name(), "star_line_mode");
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
