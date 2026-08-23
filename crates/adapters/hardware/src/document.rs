//! A printer-independent receipt: what to print, not how to say it.
//!
//! Callers build one of these and hand it to an encoder. Anything that differs between
//! ESC/POS and Star Line Mode lives in the encoders; anything a shop assistant would
//! recognise — a heading, a price column, a cut — lives here.

use crate::codepage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    #[default]
    Left,
    Centre,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextSize {
    #[default]
    Normal,
    DoubleHeight,
    DoubleWidth,
    DoubleBoth,
}

impl TextSize {
    /// How many character cells one glyph occupies horizontally. Wrapping has to know,
    /// or a double-width heading runs off the edge of the paper.
    pub(crate) fn width_multiple(self) -> usize {
        match self {
            TextSize::DoubleWidth | TextSize::DoubleBoth => 2,
            TextSize::Normal | TextSize::DoubleHeight => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextStyle {
    pub align: Alignment,
    pub bold: bool,
    pub size: TextSize,
}

impl TextStyle {
    pub fn centred() -> Self {
        Self {
            align: Alignment::Centre,
            ..Self::default()
        }
    }

    pub fn right() -> Self {
        Self {
            align: Alignment::Right,
            ..Self::default()
        }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn double(mut self) -> Self {
        self.size = TextSize::DoubleBoth;
        self
    }

    pub fn double_height(mut self) -> Self {
        self.size = TextSize::DoubleHeight;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbology {
    Code128,
    Ean13,
    Code39,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptElement {
    Text {
        content: String,
        style: TextStyle,
    },
    /// A label on the left and a value hard against the right margin: the shape of
    /// every line item and total on a receipt.
    Columns {
        left: String,
        right: String,
    },
    Divider,
    Feed(u8),
    Barcode {
        symbology: Symbology,
        data: String,
    },
    QrCode {
        data: String,
    },
    Cut,
}

/// A receipt as a sequence of elements plus the paper width in characters
/// (32 for 58mm paper, 42 or 48 for 80mm depending on the font).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptDocument {
    width_chars: usize,
    elements: Vec<ReceiptElement>,
}

impl ReceiptDocument {
    pub fn new(width_chars: usize) -> Self {
        Self {
            width_chars: width_chars.max(1),
            elements: Vec::new(),
        }
    }

    pub fn width_chars(&self) -> usize {
        self.width_chars
    }

    pub fn elements(&self) -> &[ReceiptElement] {
        &self.elements
    }

    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn push(mut self, element: ReceiptElement) -> Self {
        self.elements.push(element);
        self
    }

    pub fn text(self, content: impl Into<String>, style: TextStyle) -> Self {
        self.push(ReceiptElement::Text {
            content: content.into(),
            style,
        })
    }

    pub fn columns(self, left: impl Into<String>, right: impl Into<String>) -> Self {
        self.push(ReceiptElement::Columns {
            left: left.into(),
            right: right.into(),
        })
    }

    pub fn divider(self) -> Self {
        self.push(ReceiptElement::Divider)
    }

    pub fn feed(self, lines: u8) -> Self {
        self.push(ReceiptElement::Feed(lines))
    }

    pub fn barcode(self, symbology: Symbology, data: impl Into<String>) -> Self {
        self.push(ReceiptElement::Barcode {
            symbology,
            data: data.into(),
        })
    }

    pub fn qr(self, data: impl Into<String>) -> Self {
        self.push(ReceiptElement::QrCode { data: data.into() })
    }

    pub fn cut(self) -> Self {
        self.push(ReceiptElement::Cut)
    }

    /// The receipt as the paper will read it: laid out and wrapped to the paper width,
    /// with no printer commands. Codes are shown as their payload, since a text render
    /// cannot draw them and hiding them would misrepresent the receipt.
    ///
    /// This is the same layout the encoders use, so a line that fits here fits the paper.
    pub fn plain_text(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for element in &self.elements {
            match element {
                ReceiptElement::Text { content, style } => {
                    let width = (self.width_chars / style.size.width_multiple()).max(1);
                    for line in layout_text(content, width) {
                        let text = codepage::decode(&line);
                        lines.push(align(&text, width, style.align));
                    }
                }
                ReceiptElement::Columns { left, right } => {
                    lines.push(codepage::decode(&layout_columns(
                        left,
                        right,
                        self.width_chars,
                    )));
                }
                ReceiptElement::Divider => {
                    lines.push(codepage::decode(&layout_divider(self.width_chars)));
                }
                ReceiptElement::Feed(count) => {
                    lines.extend(std::iter::repeat_n(String::new(), *count as usize));
                }
                ReceiptElement::Barcode { symbology, data } => {
                    lines.push(format!("[{symbology:?} {data}]"));
                }
                ReceiptElement::QrCode { data } => lines.push(format!("[QR {data}]")),
                ReceiptElement::Cut => lines.push("-- cut --".into()),
            }
        }
        lines.join("\n")
    }
}

/// Centres or right-aligns a line the way the printer would, for the text render only:
/// the encoders delegate alignment to the device.
fn align(text: &str, width: usize, alignment: Alignment) -> String {
    let length = text.chars().count();
    let padding = width.saturating_sub(length);
    match alignment {
        Alignment::Left => text.to_string(),
        Alignment::Centre => format!("{}{text}", " ".repeat(padding / 2)),
        Alignment::Right => format!("{}{text}", " ".repeat(padding)),
    }
}

/// Lays a text element out into printer-code-page lines, wrapping on word boundaries.
///
/// Layout happens after transcoding so that padding is measured in the bytes the
/// printer will actually advance over: a euro sign that becomes `EUR` is three cells
/// wide, and pretending otherwise misaligns every price on the line.
pub(crate) fn layout_text(content: &str, width: usize) -> Vec<Vec<u8>> {
    let encoded = codepage::encode(content);
    if encoded.is_empty() {
        return vec![Vec::new()];
    }

    let mut lines = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    for word in encoded.split(|b| *b == b' ') {
        if word.len() > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            // A single unbreakable run longer than the paper: hard-split it rather than
            // let the printer silently drop the tail.
            for chunk in word.chunks(width) {
                lines.push(chunk.to_vec());
            }
            continue;
        }
        let projected = if current.is_empty() {
            word.len()
        } else {
            current.len() + 1 + word.len()
        };
        if projected > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(b' ');
        }
        current.extend_from_slice(word);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Pads a label and a value out to the full paper width, clipping the label when the
/// two cannot both fit. The value never moves: a receipt where the price has been
/// pushed off the paper is worthless.
pub(crate) fn layout_columns(left: &str, right: &str, width: usize) -> Vec<u8> {
    let right = codepage::encode(right);
    let mut left = codepage::encode(left);

    let room_for_left = width.saturating_sub(right.len() + 1);
    if left.len() > room_for_left {
        left.truncate(room_for_left);
    }

    let mut row = left;
    let padding = width.saturating_sub(row.len() + right.len()).max(1);
    row.extend(std::iter::repeat_n(b' ', padding));
    row.extend_from_slice(&right);
    row
}

pub(crate) fn layout_divider(width: usize) -> Vec<u8> {
    vec![b'-'; width]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_line_stays_a_blank_line() {
        assert_eq!(layout_text("", 32), vec![Vec::<u8>::new()]);
    }

    #[test]
    fn an_unbreakable_run_is_split_instead_of_dropped() {
        assert_eq!(
            layout_text("ABCDEFGHIJ", 4),
            vec![b"ABCD".to_vec(), b"EFGH".to_vec(), b"IJ".to_vec()]
        );
    }

    #[test]
    fn a_value_that_cannot_fit_still_keeps_one_space_of_separation() {
        assert_eq!(
            layout_columns("Label", "1234567890", 8),
            b" 1234567890".to_vec()
        );
    }
}
