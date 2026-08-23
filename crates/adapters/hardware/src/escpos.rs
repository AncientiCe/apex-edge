//! ESC/POS: the Epson command set, spoken by most thermal receipt printers.

use crate::document::{Alignment, ReceiptDocument, Symbology, TextSize};
use crate::encode::{encode_document, Dialect};
use crate::{HardwareError, ReceiptEncoder};

const ESC: u8 = 0x1B;
const GS: u8 = 0x1D;

/// Barcode geometry. 60 dots is tall enough to scan reliably at 203dpi without eating
/// a third of the receipt, and module width 2 keeps a long Code 128 on 58mm paper.
const BARCODE_HEIGHT_DOTS: u8 = 60;
const BARCODE_MODULE_WIDTH: u8 = 2;
const QR_MODULE_SIZE: u8 = 6;

#[derive(Debug, Clone, Copy, Default)]
pub struct EscPosEncoder;

impl ReceiptEncoder for EscPosEncoder {
    fn name(&self) -> &'static str {
        "escpos"
    }

    fn encode(&self, document: &ReceiptDocument) -> Result<Vec<u8>, HardwareError> {
        encode_document(self, document)
    }

    fn drawer_kick(&self) -> Vec<u8> {
        // ESC p m t1 t2: connector pin 2, 50ms on, 500ms off. Too short a pulse and a
        // stiff drawer does not release; too long and the solenoid cooks.
        vec![ESC, b'p', 0x00, 25, 250]
    }
}

impl Dialect for EscPosEncoder {
    fn initialise(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[ESC, b'@']);
        out.extend_from_slice(&[ESC, b't', 0x00]);
    }

    fn set_align(&self, out: &mut Vec<u8>, align: Alignment) {
        let n = match align {
            Alignment::Left => 0,
            Alignment::Centre => 1,
            Alignment::Right => 2,
        };
        out.extend_from_slice(&[ESC, b'a', n]);
    }

    fn set_bold(&self, out: &mut Vec<u8>, bold: bool) {
        out.extend_from_slice(&[ESC, b'E', u8::from(bold)]);
    }

    fn set_size(&self, out: &mut Vec<u8>, size: TextSize) {
        let n = match size {
            TextSize::Normal => 0x00,
            TextSize::DoubleHeight => 0x01,
            TextSize::DoubleWidth => 0x10,
            TextSize::DoubleBoth => 0x11,
        };
        out.extend_from_slice(&[GS, b'!', n]);
    }

    fn feed(&self, out: &mut Vec<u8>, lines: u8) {
        out.extend_from_slice(&[ESC, b'd', lines]);
    }

    fn barcode(
        &self,
        out: &mut Vec<u8>,
        symbology: Symbology,
        data: &[u8],
    ) -> Result<(), HardwareError> {
        let mut payload = Vec::with_capacity(data.len() + 2);
        if symbology == Symbology::Code128 {
            // Epson requires the code set to be selected inside the data itself.
            payload.extend_from_slice(b"{B");
        }
        payload.extend_from_slice(data);

        let length = u8::try_from(payload.len()).map_err(|_| HardwareError::InvalidPayload {
            operation: "encode_barcode".into(),
            detail: "barcode data exceeds 255 bytes".into(),
        })?;

        out.extend_from_slice(&[GS, b'h', BARCODE_HEIGHT_DOTS]);
        out.extend_from_slice(&[GS, b'w', BARCODE_MODULE_WIDTH]);
        out.extend_from_slice(&[GS, b'H', 2]); // human readable text below the bars
        out.extend_from_slice(&[GS, b'k', symbology_code(symbology), length]);
        out.extend_from_slice(&payload);
        Ok(())
    }

    fn qr(&self, out: &mut Vec<u8>, data: &[u8]) -> Result<(), HardwareError> {
        // GS ( k carries its own length, so the store command is the only one that
        // changes shape with the payload.
        let stored = u16::try_from(data.len() + 3).map_err(|_| HardwareError::InvalidPayload {
            operation: "encode_qr".into(),
            detail: "QR payload too large".into(),
        })?;

        out.extend_from_slice(&[GS, b'(', b'k', 0x04, 0x00, 0x31, 0x41, 0x32, 0x00]);
        out.extend_from_slice(&[GS, b'(', b'k', 0x03, 0x00, 0x31, 0x43, QR_MODULE_SIZE]);
        out.extend_from_slice(&[GS, b'(', b'k', 0x03, 0x00, 0x31, 0x45, 0x31]);
        out.extend_from_slice(&[
            GS,
            b'(',
            b'k',
            (stored & 0xFF) as u8,
            (stored >> 8) as u8,
            0x31,
            0x50,
            0x30,
        ]);
        out.extend_from_slice(data);
        out.extend_from_slice(&[GS, b'(', b'k', 0x03, 0x00, 0x31, 0x51, 0x30]);
        Ok(())
    }

    fn cut(&self, out: &mut Vec<u8>) {
        // GS V B 0: feed to the cutter, then partial cut so the receipt stays attached
        // until the customer takes it.
        out.extend_from_slice(&[GS, b'V', 66, 0]);
    }
}

fn symbology_code(symbology: Symbology) -> u8 {
    match symbology {
        Symbology::Ean13 => 67,
        Symbology::Code39 => 69,
        Symbology::Code128 => 73,
    }
}
