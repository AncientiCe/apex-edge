//! Where encoded bytes go: a LAN printer, a local raw port, or a test buffer.
//!
//! Every transport is fail-loud. A receipt that did not print has to surface as an
//! error the POS can show, because the alternative is an operator who thinks the
//! customer has their receipt.

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::document::ReceiptDocument;
use crate::{CashDrawer, HardwareError, PrintRequest, ReceiptEncoder, ReceiptPrinter};

pub trait PrinterTransport: Send + Sync {
    fn send(&self, bytes: &[u8]) -> Result<(), HardwareError>;
}

fn transport_error(device: &str, detail: impl std::fmt::Display) -> HardwareError {
    HardwareError::Transport {
        device: device.into(),
        detail: detail.to_string(),
    }
}

/// Raw TCP on port 9100, the near-universal network printing port. No spooler, no
/// driver: the bytes we encode are the bytes the printer receives.
#[derive(Debug, Clone)]
pub struct Tcp9100Transport {
    address: String,
    timeout: Duration,
}

impl Tcp9100Transport {
    /// `address` is `host:port`; use port 9100 unless the printer has been moved.
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            timeout: Duration::from_secs(5),
        }
    }

    /// A printer that has been switched off answers nothing at all, so every stage is
    /// bounded rather than blocking a checkout thread indefinitely.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl PrinterTransport for Tcp9100Transport {
    fn send(&self, bytes: &[u8]) -> Result<(), HardwareError> {
        let addr = self
            .address
            .to_socket_addrs()
            .map_err(|e| transport_error("tcp9100", e))?
            .next()
            .ok_or_else(|| transport_error("tcp9100", "address resolved to nothing"))?;

        let mut stream = TcpStream::connect_timeout(&addr, self.timeout)
            .map_err(|e| transport_error("tcp9100", e))?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(|e| transport_error("tcp9100", e))?;
        stream
            .write_all(bytes)
            .map_err(|e| transport_error("tcp9100", e))?;
        stream.flush().map_err(|e| transport_error("tcp9100", e))
    }
}

/// A local device path: `\\.\COM3` or a shared printer path on Windows,
/// `/dev/usb/lp0` elsewhere. Writing bytes to the port bypasses the OS spooler, which
/// is the only way to get byte-exact output through a Windows print queue.
#[derive(Debug, Clone)]
pub struct RawPortTransport {
    path: PathBuf,
}

impl RawPortTransport {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }
}

impl PrinterTransport for RawPortTransport {
    fn send(&self, bytes: &[u8]) -> Result<(), HardwareError> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.path)
            .map_err(|e| transport_error("raw_port", format!("{}: {e}", self.path.display())))?;
        file.write_all(bytes)
            .map_err(|e| transport_error("raw_port", e))?;
        file.flush().map_err(|e| transport_error("raw_port", e))
    }
}

/// Records jobs instead of printing them. This is the transport CI uses, and the one
/// that lets a POS-level test assert a receipt was produced without a printer.
#[derive(Debug, Clone, Default)]
pub struct CaptureSink {
    jobs: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl CaptureSink {
    pub fn jobs(&self) -> Vec<Vec<u8>> {
        self.jobs.lock().expect("capture sink lock").clone()
    }

    pub fn last(&self) -> Option<Vec<u8>> {
        self.jobs.lock().expect("capture sink lock").last().cloned()
    }

    pub fn clear(&self) {
        self.jobs.lock().expect("capture sink lock").clear();
    }
}

impl PrinterTransport for CaptureSink {
    fn send(&self, bytes: &[u8]) -> Result<(), HardwareError> {
        self.jobs
            .lock()
            .expect("capture sink lock")
            .push(bytes.to_vec());
        Ok(())
    }
}

/// A printer the rest of the system can hold without caring which dialect it speaks or
/// where it is plugged in.
///
/// [`TransportPrinter`] is generic over both, which is right for construction and wrong
/// for storage: the hub keeps one configured printer behind a trait object.
pub trait ReceiptDevice: Send + Sync {
    /// The dialect in use, for the `device` metrics label.
    fn encoder_name(&self) -> &'static str;

    fn print_document(&self, document: &ReceiptDocument) -> Result<(), HardwareError>;

    fn kick_drawer(&self) -> Result<(), HardwareError>;
}

impl<E: ReceiptEncoder, T: PrinterTransport> ReceiptDevice for TransportPrinter<E, T> {
    fn encoder_name(&self) -> &'static str {
        self.encoder.name()
    }

    fn print_document(&self, document: &ReceiptDocument) -> Result<(), HardwareError> {
        self.print(document)
    }

    fn kick_drawer(&self) -> Result<(), HardwareError> {
        CashDrawer::open_drawer(self)
    }
}

/// An encoder plus a transport: the thing callers actually hold.
#[derive(Debug, Clone)]
pub struct TransportPrinter<E, T> {
    encoder: E,
    transport: T,
}

impl<E: ReceiptEncoder, T: PrinterTransport> TransportPrinter<E, T> {
    pub fn new(encoder: E, transport: T) -> Self {
        Self { encoder, transport }
    }

    /// Encodes then sends. Encoding failures never reach the transport, so a malformed
    /// document cannot half-print.
    pub fn print(&self, document: &ReceiptDocument) -> Result<(), HardwareError> {
        let bytes = self.encoder.encode(document)?;
        self.transport.send(&bytes)
    }

    pub fn encoder_name(&self) -> &'static str {
        self.encoder.name()
    }
}

impl<E: ReceiptEncoder, T: PrinterTransport> ReceiptPrinter for TransportPrinter<E, T> {
    fn print_receipt(&self, request: PrintRequest) -> Result<(), HardwareError> {
        if request.bytes.is_empty() {
            return Err(HardwareError::EmptyPayload {
                operation: "print_receipt".into(),
            });
        }
        self.transport.send(&request.bytes)
    }
}

impl<E: ReceiptEncoder, T: PrinterTransport> CashDrawer for TransportPrinter<E, T> {
    fn open_drawer(&self) -> Result<(), HardwareError> {
        self.transport.send(&self.encoder.drawer_kick())
    }
}
