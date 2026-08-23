//! Transport tests: where the encoded bytes actually go.
//!
//! Nothing here needs a printer. The TCP tests stand up a listener that plays the part
//! of a networked Star or Epson on port 9100, the raw port test writes to a temporary
//! file the way it would write to `\\.\COM3` or `/dev/usb/lp0`, and `CaptureSink` is
//! what CI and the POS integration tests assert against.

use std::io::Read;
use std::net::TcpListener;
use std::sync::mpsc;

use apex_edge_adapters_hardware::{
    CaptureSink, CashDrawer, EscPosEncoder, HardwareError, PrintRequest, PrinterTransport,
    RawPortTransport, ReceiptDocument, ReceiptPrinter, StarLineModeEncoder, Tcp9100Transport,
    TextStyle, TransportPrinter,
};

fn receipt() -> ReceiptDocument {
    ReceiptDocument::new(32)
        .text("APEX STORE", TextStyle::centred().bold())
        .columns("TOTAL", "3.50")
        .cut()
}

#[test]
fn the_capture_sink_keeps_every_job_for_assertions() {
    let sink = CaptureSink::default();

    sink.send(b"first").expect("send");
    sink.send(b"second").expect("send");

    assert_eq!(sink.jobs(), vec![b"first".to_vec(), b"second".to_vec()]);
    assert_eq!(sink.last().expect("a job"), b"second".to_vec());
}

#[test]
fn a_receipt_reaches_a_networked_printer_on_port_9100() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).expect("read");
        tx.send(buf).expect("send");
    });

    let printer = TransportPrinter::new(EscPosEncoder, Tcp9100Transport::new(addr.to_string()));
    printer.print(&receipt()).expect("print");

    let received = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("printer received the job");
    assert!(received.starts_with(&[0x1B, 0x40]), "job must initialise");
    assert!(
        received.windows(4).any(|w| w == [0x1D, 0x56, 0x42, 0x00]),
        "job must cut the paper"
    );
}

#[test]
fn an_unreachable_printer_is_reported_rather_than_silently_swallowed() {
    // Port 1 on loopback: nothing listens, and the store needs to be told the receipt
    // did not print instead of the sale quietly continuing.
    let printer = TransportPrinter::new(StarLineModeEncoder, Tcp9100Transport::new("127.0.0.1:1"));

    let err = printer.print(&receipt()).expect_err("must fail");

    assert!(
        matches!(err, HardwareError::Transport { ref device, .. } if device == "tcp9100"),
        "expected a transport error naming the device, got {err:?}"
    );
}

#[test]
fn a_raw_port_transport_writes_the_bytes_to_the_device_path() {
    let dir = std::env::temp_dir().join(format!("apex-printer-{}", uuid_like()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("lp0");

    let printer = TransportPrinter::new(EscPosEncoder, RawPortTransport::new(&path));
    printer.print(&receipt()).expect("print");

    let written = std::fs::read(&path).expect("read back");
    assert!(written.starts_with(&[0x1B, 0x40]));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_raw_port_that_cannot_be_opened_is_a_transport_error() {
    let printer = TransportPrinter::new(
        EscPosEncoder,
        RawPortTransport::new("no-such-directory/does/not/exist/lp0"),
    );

    let err = printer.print(&receipt()).expect_err("must fail");

    assert!(
        matches!(err, HardwareError::Transport { ref device, .. } if device == "raw_port"),
        "got {err:?}"
    );
}

#[test]
fn an_empty_document_never_reaches_the_transport() {
    let sink = CaptureSink::default();
    let printer = TransportPrinter::new(EscPosEncoder, sink.clone());

    let err = printer
        .print(&ReceiptDocument::new(32))
        .expect_err("must fail");

    assert_eq!(
        err,
        HardwareError::EmptyPayload {
            operation: "encode_receipt".into()
        }
    );
    assert!(sink.jobs().is_empty(), "no paper should have moved");
}

#[test]
fn the_drawer_kick_goes_out_over_the_same_transport() {
    let sink = CaptureSink::default();
    let printer = TransportPrinter::new(EscPosEncoder, sink.clone());

    printer.open_drawer().expect("kick");

    assert_eq!(
        sink.last().expect("a job"),
        vec![0x1B, 0x70, 0x00, 0x19, 0xFA]
    );
}

#[test]
fn pre_encoded_bytes_still_go_out_through_the_receipt_printer_trait() {
    // The existing ReceiptPrinter contract takes bytes, and callers that already have
    // an encoded document must keep working.
    let sink = CaptureSink::default();
    let printer = TransportPrinter::new(EscPosEncoder, sink.clone());

    printer
        .print_receipt(PrintRequest {
            document_type: "receipt".into(),
            bytes: b"raw".to_vec(),
        })
        .expect("print");

    assert_eq!(sink.last().expect("a job"), b"raw".to_vec());
}

#[test]
fn a_receipt_printer_still_refuses_an_empty_byte_payload() {
    let sink = CaptureSink::default();
    let printer = TransportPrinter::new(EscPosEncoder, sink.clone());

    assert_eq!(
        printer.print_receipt(PrintRequest {
            document_type: "receipt".into(),
            bytes: Vec::new(),
        }),
        Err(HardwareError::EmptyPayload {
            operation: "print_receipt".into()
        })
    );
    assert!(sink.jobs().is_empty());
}

fn uuid_like() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}
