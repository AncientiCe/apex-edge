//! The shared walk over a receipt document.
//!
//! ESC/POS and Star Line Mode disagree about how to say things, not about what a
//! receipt is. Keeping the traversal — wrapping, column padding, and tracking which
//! styles the printer is already in — in one place means the two dialects cannot drift
//! apart in their layout, only in their bytes.

use crate::document::{
    layout_columns, layout_divider, layout_text, Alignment, ReceiptDocument, ReceiptElement,
    Symbology, TextSize, TextStyle,
};
use crate::HardwareError;

pub(crate) trait Dialect {
    /// Reset the printer and select the code page text is transcoded to.
    fn initialise(&self, out: &mut Vec<u8>);
    fn set_align(&self, out: &mut Vec<u8>, align: Alignment);
    fn set_bold(&self, out: &mut Vec<u8>, bold: bool);
    fn set_size(&self, out: &mut Vec<u8>, size: TextSize);
    fn feed(&self, out: &mut Vec<u8>, lines: u8);
    fn barcode(
        &self,
        out: &mut Vec<u8>,
        symbology: Symbology,
        data: &[u8],
    ) -> Result<(), HardwareError>;
    fn qr(&self, out: &mut Vec<u8>, data: &[u8]) -> Result<(), HardwareError>;
    fn cut(&self, out: &mut Vec<u8>);
}

/// The style the printer is in. A printer remembers what it was last told, so emitting
/// a command that changes nothing is wasted bytes, and forgetting to emit one that does
/// prints the rest of the receipt in the wrong style.
struct PrinterStyle {
    align: Alignment,
    bold: bool,
    size: TextSize,
}

impl PrinterStyle {
    /// The state a printer is in immediately after being initialised.
    fn initial() -> Self {
        Self {
            align: Alignment::Left,
            bold: false,
            size: TextSize::Normal,
        }
    }

    fn apply<D: Dialect>(&mut self, dialect: &D, out: &mut Vec<u8>, wanted: TextStyle) {
        if self.align != wanted.align {
            dialect.set_align(out, wanted.align);
            self.align = wanted.align;
        }
        if self.bold != wanted.bold {
            dialect.set_bold(out, wanted.bold);
            self.bold = wanted.bold;
        }
        if self.size != wanted.size {
            dialect.set_size(out, wanted.size);
            self.size = wanted.size;
        }
    }
}

pub(crate) fn encode_document<D: Dialect>(
    dialect: &D,
    doc: &ReceiptDocument,
) -> Result<Vec<u8>, HardwareError> {
    if doc.is_empty() {
        // Cutting blank paper on every failed render is how a roll disappears in a day.
        return Err(HardwareError::EmptyPayload {
            operation: "encode_receipt".into(),
        });
    }

    let width = doc.width_chars();
    let mut out = Vec::new();
    let mut style = PrinterStyle::initial();
    dialect.initialise(&mut out);

    for element in doc.elements() {
        match element {
            ReceiptElement::Text {
                content,
                style: wanted,
            } => {
                style.apply(dialect, &mut out, *wanted);
                for line in layout_text(content, width / wanted.size.width_multiple()) {
                    out.extend_from_slice(&line);
                    out.push(b'\n');
                }
            }
            ReceiptElement::Columns { left, right } => {
                style.apply(dialect, &mut out, TextStyle::default());
                out.extend_from_slice(&layout_columns(left, right, width));
                out.push(b'\n');
            }
            ReceiptElement::Divider => {
                style.apply(dialect, &mut out, TextStyle::default());
                out.extend_from_slice(&layout_divider(width));
                out.push(b'\n');
            }
            ReceiptElement::Feed(lines) => dialect.feed(&mut out, *lines),
            ReceiptElement::Barcode { symbology, data } => {
                style.apply(dialect, &mut out, TextStyle::centred());
                let data = validate_barcode(*symbology, data)?;
                dialect.barcode(&mut out, *symbology, &data)?;
            }
            ReceiptElement::QrCode { data } => {
                style.apply(dialect, &mut out, TextStyle::centred());
                if data.is_empty() {
                    return Err(HardwareError::EmptyPayload {
                        operation: "encode_qr".into(),
                    });
                }
                dialect.qr(&mut out, data.as_bytes())?;
            }
            ReceiptElement::Cut => dialect.cut(&mut out),
        }
    }

    Ok(out)
}

/// A barcode the scanner cannot read is indistinguishable from a missing barcode, and
/// the failure shows up at a returns desk weeks later. Reject bad payloads at encode
/// time, where there is still someone to tell.
fn validate_barcode(symbology: Symbology, data: &str) -> Result<Vec<u8>, HardwareError> {
    if data.is_empty() {
        return Err(HardwareError::EmptyPayload {
            operation: "encode_barcode".into(),
        });
    }
    if !data.is_ascii() {
        return Err(HardwareError::InvalidPayload {
            operation: "encode_barcode".into(),
            detail: "barcode data must be ASCII".into(),
        });
    }
    match symbology {
        Symbology::Code128 | Symbology::Code39 => {}
        Symbology::Ean13 => {
            let digits = data.chars().all(|c| c.is_ascii_digit());
            if !digits || !(12..=13).contains(&data.len()) {
                return Err(HardwareError::InvalidPayload {
                    operation: "encode_barcode".into(),
                    detail: "EAN-13 needs 12 or 13 digits".into(),
                });
            }
        }
    }
    Ok(data.as_bytes().to_vec())
}
