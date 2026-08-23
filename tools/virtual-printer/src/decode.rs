//! Turns a printer byte stream back into a receipt.
//!
//! This is the inverse of `crates/adapters/hardware`'s encoders, and it exists so a
//! human can see what a printer would have produced. It is also the honest test of the
//! encoders: if the decoder cannot make sense of a byte, a real printer probably could
//! not either, and it says so in `warnings` rather than guessing.

use apex_edge_adapters_hardware::codepage;
use serde::Serialize;

const ESC: u8 = 0x1B;
const GS: u8 = 0x1D;
const BEL: u8 = 0x07;
const RS: u8 = 0x1E;
const LF: u8 = 0x0A;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    #[serde(rename = "escpos")]
    EscPos,
    StarLineMode,
    Unknown,
}

impl Dialect {
    pub fn label(self) -> &'static str {
        match self {
            Dialect::EscPos => "escpos",
            Dialect::StarLineMode => "star_line_mode",
            Dialect::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Alignment {
    #[default]
    Left,
    Centre,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct LineStyle {
    pub align: Alignment,
    pub bold: bool,
    pub double_width: bool,
    pub double_height: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecodedElement {
    Line { text: String, style: LineStyle },
    Feed { lines: u8 },
    Barcode { symbology: String, data: String },
    QrCode { data: String },
    Cut,
    DrawerKick,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecodedJob {
    pub dialect: Dialect,
    pub elements: Vec<DecodedElement>,
    pub warnings: Vec<String>,
}

impl DecodedJob {
    /// The text a customer would read off the paper. Barcodes and QR codes are not
    /// text and are reported as their own elements instead.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for element in &self.elements {
            match element {
                DecodedElement::Line { text, .. } => {
                    out.push_str(text);
                    out.push('\n');
                }
                DecodedElement::Feed { lines } => {
                    for _ in 0..*lines {
                        out.push('\n');
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// Decodes one print job.
///
/// The dialect has to be settled before parsing because the two command sets collide:
/// `ESC d n` feeds n lines on an ESC/POS printer and cuts the paper on a Star. Getting
/// that backwards is exactly the class of bug this tool exists to catch.
pub fn decode_job(bytes: &[u8]) -> DecodedJob {
    let dialect = detect_dialect(bytes);
    let mut decoder = Decoder {
        dialect,
        elements: Vec::new(),
        warnings: Vec::new(),
        line: Vec::new(),
        style: LineStyle::default(),
    };
    decoder.run(bytes);
    DecodedJob {
        dialect,
        elements: decoder.elements,
        warnings: decoder.warnings,
    }
}

fn detect_dialect(bytes: &[u8]) -> Dialect {
    if bytes.is_empty() {
        return Dialect::Unknown;
    }
    // Both encoders select a code page immediately after ESC @, and that command is
    // the cleanest fingerprint: ESC t for Epson, ESC GS t for Star.
    if bytes.windows(2).any(|w| w == [ESC, GS]) {
        return Dialect::StarLineMode;
    }
    if bytes
        .windows(2)
        .any(|w| w == [ESC, b't'] || w == [GS, b'!'])
    {
        return Dialect::EscPos;
    }
    if bytes.windows(2).any(|w| w == [ESC, b'p']) {
        return Dialect::EscPos;
    }
    if bytes.first() == Some(&ESC) && bytes.get(1) == Some(&BEL) {
        return Dialect::StarLineMode;
    }
    Dialect::Unknown
}

struct Decoder {
    dialect: Dialect,
    elements: Vec<DecodedElement>,
    warnings: Vec<String>,
    line: Vec<u8>,
    style: LineStyle,
}

impl Decoder {
    fn run(&mut self, bytes: &[u8]) {
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            let consumed = match bytes[cursor] {
                LF => {
                    self.flush_line();
                    1
                }
                ESC | GS | BEL => self.command(&bytes[cursor..]),
                byte => {
                    self.line.push(byte);
                    1
                }
            };
            cursor += consumed.max(1);
        }
        // A stream that ends without a line feed still printed something.
        if !self.line.is_empty() {
            self.flush_line();
        }
    }

    fn flush_line(&mut self) {
        let text = codepage::decode(&std::mem::take(&mut self.line));
        self.elements.push(DecodedElement::Line {
            text,
            style: self.style,
        });
    }

    /// Returns how many bytes the command consumed, or 0 if it was not recognised.
    fn command(&mut self, rest: &[u8]) -> usize {
        let consumed = match self.dialect {
            Dialect::StarLineMode => self.star_command(rest),
            _ => self.escpos_command(rest),
        };
        if consumed == 0 {
            let width = rest.len().min(2);
            self.warnings.push(format!(
                "unrecognised command {}",
                hex(&rest[..width.max(1)])
            ));
            return width.max(1);
        }
        consumed
    }

    fn escpos_command(&mut self, rest: &[u8]) -> usize {
        match rest {
            [ESC, b'@', ..] => {
                self.style = LineStyle::default();
                2
            }
            [ESC, b't', _, ..] => 3,
            [ESC, b'a', n, ..] => {
                self.style.align = alignment(*n);
                3
            }
            [ESC, b'E', n, ..] => {
                self.style.bold = *n != 0;
                3
            }
            [GS, b'!', n, ..] => {
                self.style.double_height = n & 0x01 != 0;
                self.style.double_width = n & 0x10 != 0;
                3
            }
            [ESC, b'd', n, ..] => {
                self.elements.push(DecodedElement::Feed { lines: *n });
                3
            }
            [ESC, b'p', _, _, _, ..] => {
                self.elements.push(DecodedElement::DrawerKick);
                5
            }
            [GS, b'V', 66, _, ..] => {
                self.elements.push(DecodedElement::Cut);
                4
            }
            [GS, b'V', _, ..] => {
                self.elements.push(DecodedElement::Cut);
                3
            }
            // Barcode geometry: no effect on what the receipt says.
            [GS, b'h', _, ..] | [GS, b'w', _, ..] | [GS, b'H', _, ..] => 3,
            [GS, b'k', symbology, length, ..] if *symbology >= 65 => {
                let start = 4;
                let end = start + usize::from(*length);
                if end > rest.len() {
                    self.warnings
                        .push("barcode data ran past the end of the job".into());
                    return rest.len();
                }
                self.push_barcode(escpos_symbology(*symbology), &rest[start..end]);
                end
            }
            [GS, b'(', b'k', low, high, ..] => self.escpos_qr(rest, *low, *high),
            _ => 0,
        }
    }

    /// `GS ( k` carries its own length, so the payload is found the same way for every
    /// function; only the store function (80) actually holds the QR data.
    fn escpos_qr(&mut self, rest: &[u8], low: u8, high: u8) -> usize {
        let length = usize::from(u16::from_le_bytes([low, high]));
        let end = 5 + length;
        if end > rest.len() || length < 2 {
            self.warnings
                .push("QR command ran past the end of the job".into());
            return rest.len();
        }
        let function = rest[6];
        if function == 0x50 {
            let data = &rest[8..end];
            self.elements.push(DecodedElement::QrCode {
                data: codepage::decode(data),
            });
        }
        end
    }

    fn star_command(&mut self, rest: &[u8]) -> usize {
        match rest {
            [ESC, b'@', ..] => {
                self.style = LineStyle::default();
                2
            }
            [ESC, GS, b't', _, ..] => 4,
            [ESC, GS, b'a', n, ..] => {
                self.style.align = alignment(*n);
                4
            }
            [ESC, b'E', ..] => {
                self.style.bold = true;
                2
            }
            [ESC, b'F', ..] => {
                self.style.bold = false;
                2
            }
            [ESC, b'i', height, width, ..] => {
                self.style.double_height = *height != 0;
                self.style.double_width = *width != 0;
                4
            }
            [ESC, b'd', _, ..] => {
                self.elements.push(DecodedElement::Cut);
                3
            }
            [ESC, BEL, _, _, ..] => 4,
            [BEL, ..] => {
                self.elements.push(DecodedElement::DrawerKick);
                1
            }
            [ESC, b'b', symbology, _, _, _, ..] => {
                let start = 6;
                let Some(offset) = rest[start..].iter().position(|b| *b == RS) else {
                    self.warnings
                        .push("barcode data was never terminated with RS".into());
                    return rest.len();
                };
                self.push_barcode(star_symbology(*symbology), &rest[start..start + offset]);
                start + offset + 1
            }
            [ESC, GS, b'y', b'S', _, _, ..] => 6,
            [ESC, GS, b'y', b'D', b'1', _, low, high, ..] => {
                let length = usize::from(u16::from_le_bytes([*low, *high]));
                let end = 8 + length;
                if end > rest.len() {
                    self.warnings
                        .push("QR data ran past the end of the job".into());
                    return rest.len();
                }
                self.elements.push(DecodedElement::QrCode {
                    data: codepage::decode(&rest[8..end]),
                });
                end
            }
            [ESC, GS, b'y', b'P', ..] => 4,
            _ => 0,
        }
    }

    fn push_barcode(&mut self, symbology: &str, data: &[u8]) {
        // Epson needs the code set selected inside the data; that is a wire detail, not
        // something that appears under the bars.
        let decoded = codepage::decode(data);
        let data = decoded.strip_prefix("{B").unwrap_or(&decoded).to_string();
        self.elements.push(DecodedElement::Barcode {
            symbology: symbology.to_string(),
            data,
        });
    }
}

fn alignment(n: u8) -> Alignment {
    match n {
        1 => Alignment::Centre,
        2 => Alignment::Right,
        _ => Alignment::Left,
    }
}

fn escpos_symbology(code: u8) -> &'static str {
    match code {
        67 => "Ean13",
        69 => "Code39",
        73 => "Code128",
        _ => "unknown",
    }
}

fn star_symbology(code: u8) -> &'static str {
    match code {
        3 => "Ean13",
        4 => "Code39",
        6 => "Code128",
        _ => "unknown",
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
