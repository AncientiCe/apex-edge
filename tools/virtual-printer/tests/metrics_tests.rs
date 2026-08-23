//! The virtual printer is observable for the same reason the hub is: "did the receipt
//! arrive" has to be answerable without reading logs.
//!
//! This lives in its own test binary because installing a metrics recorder is a
//! process-global act.

use apex_edge_adapters_hardware::{EscPosEncoder, ReceiptDocument, ReceiptEncoder, TextStyle};
use metrics_exporter_prometheus::PrometheusBuilder;
use virtual_printer::JobStore;

#[test]
fn received_jobs_bytes_and_decode_warnings_are_all_counted() {
    let handle = PrometheusBuilder::new()
        .install_recorder()
        .expect("install recorder");
    let store = JobStore::default();

    let good = EscPosEncoder
        .encode(&ReceiptDocument::new(32).text("APEX STORE", TextStyle::centred()))
        .expect("encode");
    store.record(&good);
    // A stream carrying a command no printer implements: worth a metric, because it is
    // how a broken encoder change gets noticed.
    store.record(&[0x1B, 0x40, 0x1B, 0x74, 0x00, 0x1B, 0x5A, b'x', 0x0A]);

    let rendered = handle.render();

    assert!(
        rendered.contains("virtual_printer_jobs_total"),
        "no job counter in {rendered}"
    );
    assert!(
        rendered.contains("dialect=\"escpos\""),
        "the dialect must be labelled: {rendered}"
    );
    assert!(
        rendered.contains("virtual_printer_bytes_total"),
        "no byte counter in {rendered}"
    );
    assert!(
        rendered.contains("virtual_printer_decode_warnings_total"),
        "no warning counter in {rendered}"
    );
}
