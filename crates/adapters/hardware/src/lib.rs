//! Hardware adapter traits and sidecar-friendly reference implementations.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintRequest {
    pub document_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarcodeScan {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaleReading {
    pub weight_millis: i64,
    pub unit: String,
    pub stable: bool,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum HardwareError {
    #[error("hardware device {device} is not configured")]
    NotConfigured { device: String },
    #[error("hardware operation {operation} has no data")]
    EmptyPayload { operation: String },
}

pub trait ReceiptPrinter: Send + Sync {
    fn print_receipt(&self, request: PrintRequest) -> Result<(), HardwareError>;
}

pub trait CashDrawer: Send + Sync {
    fn open_drawer(&self) -> Result<(), HardwareError>;
}

pub trait BarcodeScanner: Send + Sync {
    fn read_barcode(&self) -> Result<Option<BarcodeScan>, HardwareError>;
}

pub trait WeightScale: Send + Sync {
    fn read_scale(&self) -> Result<Option<ScaleReading>, HardwareError>;
}

pub trait CustomerDisplay: Send + Sync {
    fn display_line(&self, text: &str) -> Result<(), HardwareError>;
}

#[derive(Debug, Clone)]
pub struct EscPosHardwareProvider {
    configured: bool,
}

impl EscPosHardwareProvider {
    pub fn new(configured: bool) -> Self {
        Self { configured }
    }
}

impl ReceiptPrinter for EscPosHardwareProvider {
    fn print_receipt(&self, request: PrintRequest) -> Result<(), HardwareError> {
        validate_configured("escpos_printer", self.configured)?;
        if request.bytes.is_empty() {
            return Err(HardwareError::EmptyPayload {
                operation: "print_receipt".into(),
            });
        }
        Ok(())
    }
}

impl CashDrawer for EscPosHardwareProvider {
    fn open_drawer(&self) -> Result<(), HardwareError> {
        validate_configured("cash_drawer", self.configured)
    }
}

#[derive(Debug, Clone, Default)]
pub struct HidBarcodeScanner {
    next_scan: Option<BarcodeScan>,
}

impl HidBarcodeScanner {
    pub fn with_next_scan(value: impl Into<String>) -> Self {
        Self {
            next_scan: Some(BarcodeScan {
                value: value.into(),
            }),
        }
    }
}

impl BarcodeScanner for HidBarcodeScanner {
    fn read_barcode(&self) -> Result<Option<BarcodeScan>, HardwareError> {
        Ok(self.next_scan.clone())
    }
}

#[derive(Debug, Clone, Default)]
pub struct NciWeightScale {
    reading: Option<ScaleReading>,
}

impl NciWeightScale {
    pub fn with_reading(weight_millis: i64, unit: impl Into<String>, stable: bool) -> Self {
        Self {
            reading: Some(ScaleReading {
                weight_millis,
                unit: unit.into(),
                stable,
            }),
        }
    }
}

impl WeightScale for NciWeightScale {
    fn read_scale(&self) -> Result<Option<ScaleReading>, HardwareError> {
        Ok(self.reading.clone())
    }
}

#[derive(Debug, Clone)]
pub struct TextCustomerDisplay {
    configured: bool,
}

impl TextCustomerDisplay {
    pub fn new(configured: bool) -> Self {
        Self { configured }
    }
}

impl CustomerDisplay for TextCustomerDisplay {
    fn display_line(&self, text: &str) -> Result<(), HardwareError> {
        validate_configured("customer_display", self.configured)?;
        if text.trim().is_empty() {
            return Err(HardwareError::EmptyPayload {
                operation: "display_line".into(),
            });
        }
        Ok(())
    }
}

fn validate_configured(device: &str, configured: bool) -> Result<(), HardwareError> {
    if !configured {
        return Err(HardwareError::NotConfigured {
            device: device.into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escpos_printer_and_drawer_fail_closed_when_unconfigured() {
        let provider = EscPosHardwareProvider::new(false);

        assert_eq!(
            provider.print_receipt(PrintRequest {
                document_type: "receipt".into(),
                bytes: b"hello".to_vec(),
            }),
            Err(HardwareError::NotConfigured {
                device: "escpos_printer".into()
            })
        );
        assert_eq!(
            provider.open_drawer(),
            Err(HardwareError::NotConfigured {
                device: "cash_drawer".into()
            })
        );
    }

    #[test]
    fn scanner_scale_and_display_return_reference_data() {
        let scanner = HidBarcodeScanner::with_next_scan("012345678905");
        let scale = NciWeightScale::with_reading(1_250, "g", true);
        let display = TextCustomerDisplay::new(true);

        assert_eq!(
            scanner.read_barcode().expect("scanner read"),
            Some(BarcodeScan {
                value: "012345678905".into()
            })
        );
        assert_eq!(
            scale.read_scale().expect("scale read"),
            Some(ScaleReading {
                weight_millis: 1_250,
                unit: "g".into(),
                stable: true
            })
        );
        display.display_line("Total 12.50").expect("display line");
    }
}
