//! Star Line Mode: the command set Star TSP-series printers speak natively.
//!
//! Star hardware is common in retail and is deliberately *not* ESC/POS in line mode:
//! alignment, character expansion, cutting and the drawer are all different commands.
//! Sending ESC/POS to a Star in line mode mostly prints, which is what makes the bug so
//! easy to miss.

use crate::document::{Alignment, ReceiptDocument, Symbology, TextSize};
use crate::encode::{encode_document, Dialect};
use crate::{HardwareError, ReceiptEncoder};

const ESC: u8 = 0x1B;
const GS: u8 = 0x1D;
const BEL: u8 = 0x07;
const RS: u8 = 0x1E;

const BARCODE_HEIGHT_DOTS: u8 = 60;
const BARCODE_WIDTH_MODE: u8 = 2;
const QR_CELL_SIZE: u8 = 6;

#[derive(Debug, Clone, Copy, Default)]
pub struct StarLineModeEncoder;

impl ReceiptEncoder for StarLineModeEncoder {
    fn name(&self) -> &'static str {
        "star_line_mode"
    }

    fn encode(&self, document: &ReceiptDocument) -> Result<Vec<u8>, HardwareError> {
        encode_document(self, document)
    }

    fn drawer_kick(&self) -> Vec<u8> {
        // ESC BEL n1 n2 sets the pulse for drawer 1 in 10ms units; the bare BEL fires it.
        vec![ESC, BEL, 5, 50, BEL]
    }
}

impl Dialect for StarLineModeEncoder {
    fn initialise(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[ESC, b'@']);
        out.extend_from_slice(&[ESC, GS, b't', 0x00]);
    }

    fn set_align(&self, out: &mut Vec<u8>, align: Alignment) {
        let n = match align {
            Alignment::Left => 0,
            Alignment::Centre => 1,
            Alignment::Right => 2,
        };
        out.extend_from_slice(&[ESC, GS, b'a', n]);
    }

    fn set_bold(&self, out: &mut Vec<u8>, bold: bool) {
        // Star has no parameter here: emphasis on and off are two separate commands.
        out.extend_from_slice(&[ESC, if bold { b'E' } else { b'F' }]);
    }

    fn set_size(&self, out: &mut Vec<u8>, size: TextSize) {
        let (height, width) = match size {
            TextSize::Normal => (0, 0),
            TextSize::DoubleHeight => (1, 0),
            TextSize::DoubleWidth => (0, 1),
            TextSize::DoubleBoth => (1, 1),
        };
        out.extend_from_slice(&[ESC, b'i', height, width]);
    }

    fn feed(&self, out: &mut Vec<u8>, lines: u8) {
        // There is no ESC d equivalent in line mode; blank lines are just line feeds.
        out.extend(std::iter::repeat_n(b'\n', usize::from(lines)));
    }

    fn barcode(
        &self,
        out: &mut Vec<u8>,
        symbology: Symbology,
        data: &[u8],
    ) -> Result<(), HardwareError> {
        if data.contains(&RS) {
            // RS terminates the data, so a payload containing it would truncate the
            // barcode and leave the rest of the bytes printing as text.
            return Err(HardwareError::InvalidPayload {
                operation: "encode_barcode".into(),
                detail: "barcode data must not contain RS".into(),
            });
        }
        out.extend_from_slice(&[
            ESC,
            b'b',
            symbology_code(symbology),
            2, // print the human readable text under the bars
            BARCODE_WIDTH_MODE,
            BARCODE_HEIGHT_DOTS,
        ]);
        out.extend_from_slice(data);
        out.push(RS);
        Ok(())
    }

    fn qr(&self, out: &mut Vec<u8>, data: &[u8]) -> Result<(), HardwareError> {
        let length = u16::try_from(data.len()).map_err(|_| HardwareError::InvalidPayload {
            operation: "encode_qr".into(),
            detail: "QR payload too large".into(),
        })?;

        out.extend_from_slice(&[ESC, GS, b'y', b'S', b'0', 0x02]); // model 2
        out.extend_from_slice(&[ESC, GS, b'y', b'S', b'1', 0x01]); // error correction M
        out.extend_from_slice(&[ESC, GS, b'y', b'S', b'2', QR_CELL_SIZE]);
        out.extend_from_slice(&[
            ESC,
            GS,
            b'y',
            b'D',
            b'1',
            0x00,
            (length & 0xFF) as u8,
            (length >> 8) as u8,
        ]);
        out.extend_from_slice(data);
        out.extend_from_slice(&[ESC, GS, b'y', b'P']);
        Ok(())
    }

    fn cut(&self, out: &mut Vec<u8>) {
        // ESC d 3: feed to the cutter and partial cut, matching the ESC/POS behaviour.
        out.extend_from_slice(&[ESC, b'd', 3]);
    }
}

fn symbology_code(symbology: Symbology) -> u8 {
    match symbology {
        Symbology::Ean13 => 3,
        Symbology::Code39 => 4,
        Symbology::Code128 => 6,
    }
}
