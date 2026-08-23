//! The virtual printer is how anyone who clones this repo sees a receipt without owning
//! a printer, so its decoder is held to the same standard as the encoders: real byte
//! streams in, a readable receipt out.

use apex_edge_adapters_hardware::{
    EscPosEncoder, ReceiptDocument, ReceiptEncoder, StarLineModeEncoder, Symbology,
    Tcp9100Transport, TextStyle, TransportPrinter,
};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use virtual_printer::{decode_job, Alignment, DecodedElement, Dialect, JobStore};

fn receipt() -> ReceiptDocument {
    ReceiptDocument::new(32)
        .text("APEX STORE", TextStyle::centred().bold().double())
        .divider()
        .columns("Café", "3.50")
        .columns("TOTAL", "3.50")
        .qr("https://apex.example/r/1001")
        .barcode(Symbology::Code128, "1001")
        .feed(2)
        .cut()
}

#[test]
fn an_escpos_stream_decodes_back_into_the_receipt_that_was_printed() {
    let bytes = EscPosEncoder.encode(&receipt()).expect("encode");

    let job = decode_job(&bytes);

    let expected = format!(
        "APEX STORE\n{divider}\nCafé{gap}3.50\nTOTAL{wider}3.50\n\n\n",
        divider = "-".repeat(32),
        gap = " ".repeat(24),
        wider = " ".repeat(23),
    );

    assert_eq!(job.dialect, Dialect::EscPos);
    assert_eq!(job.text(), expected);
    assert!(job.warnings.is_empty(), "unexpected {:?}", job.warnings);
}

#[test]
fn a_star_line_mode_stream_decodes_into_the_same_receipt() {
    // The whole point of two encoders is that they print the same receipt. If the
    // decoder shows a difference, one of the encoders is wrong.
    let escpos = decode_job(&EscPosEncoder.encode(&receipt()).expect("encode"));
    let star = decode_job(&StarLineModeEncoder.encode(&receipt()).expect("encode"));

    assert_eq!(star.dialect, Dialect::StarLineMode);
    assert_eq!(star.text(), escpos.text());
    assert!(star.warnings.is_empty(), "unexpected {:?}", star.warnings);
}

#[test]
fn styling_survives_the_round_trip() {
    let job = decode_job(&EscPosEncoder.encode(&receipt()).expect("encode"));

    let heading = job
        .elements
        .iter()
        .find_map(|element| match element {
            DecodedElement::Line { text, style } if text == "APEX STORE" => Some(style),
            _ => None,
        })
        .expect("heading line");

    assert!(heading.bold, "the heading was printed bold");
    assert!(heading.double_width && heading.double_height);
    assert_eq!(heading.align, Alignment::Centre);
}

#[test]
fn a_qr_payload_is_recovered_so_it_can_be_read_off_the_screen() {
    // Fiscal receipts carry their signature in the QR code; being able to read the
    // payload back is how that gets verified without a phone camera.
    for bytes in [
        EscPosEncoder.encode(&receipt()).expect("encode"),
        StarLineModeEncoder.encode(&receipt()).expect("encode"),
    ] {
        let job = decode_job(&bytes);
        assert!(
            job.elements.iter().any(|e| matches!(
                e,
                DecodedElement::QrCode { data } if data == "https://apex.example/r/1001"
            )),
            "no QR payload in {:?}",
            job.elements
        );
    }
}

#[test]
fn a_barcode_is_recovered_with_its_symbology_and_data() {
    for bytes in [
        EscPosEncoder.encode(&receipt()).expect("encode"),
        StarLineModeEncoder.encode(&receipt()).expect("encode"),
    ] {
        let job = decode_job(&bytes);
        assert!(
            job.elements.iter().any(|e| matches!(
                e,
                DecodedElement::Barcode { symbology, data }
                    if symbology == "Code128" && data == "1001"
            )),
            "no barcode in {:?}",
            job.elements
        );
    }
}

#[test]
fn the_cut_is_reported_so_a_missing_one_is_visible() {
    for bytes in [
        EscPosEncoder.encode(&receipt()).expect("encode"),
        StarLineModeEncoder.encode(&receipt()).expect("encode"),
    ] {
        let job = decode_job(&bytes);
        assert!(
            matches!(job.elements.last(), Some(DecodedElement::Cut)),
            "the paper should be cut last, got {:?}",
            job.elements.last()
        );
    }
}

#[test]
fn a_drawer_kick_is_reported_even_though_it_prints_nothing() {
    for bytes in [
        EscPosEncoder.drawer_kick(),
        StarLineModeEncoder.drawer_kick(),
    ] {
        let job = decode_job(&bytes);
        assert!(
            job.elements
                .iter()
                .any(|e| matches!(e, DecodedElement::DrawerKick)),
            "no drawer kick in {:?}",
            job.elements
        );
    }
}

#[test]
fn an_unknown_command_is_reported_instead_of_printed_as_text() {
    // Garbage on the paper is how you learn a command was wrong; a warning is how you
    // learn which byte it was.
    let job = decode_job(&[0x1B, 0x40, 0x1B, 0x74, 0x00, 0x1B, 0x5A, b'h', b'i', 0x0A]);

    assert_eq!(job.text(), "hi\n", "the stray command must not print");
    assert_eq!(job.warnings.len(), 1);
    assert!(
        job.warnings[0].contains("1B 5A"),
        "the warning should name the bytes, got {:?}",
        job.warnings
    );
}

#[test]
fn an_empty_stream_is_not_a_receipt() {
    let job = decode_job(&[]);

    assert_eq!(job.dialect, Dialect::Unknown);
    assert!(job.elements.is_empty());
}

#[tokio::test]
async fn a_job_arriving_on_the_wire_shows_up_in_the_api() {
    let store = JobStore::default();
    store.record(&EscPosEncoder.encode(&receipt()).expect("encode"));

    let app = virtual_printer::build_app(store);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/jobs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["jobs"].as_array().expect("jobs").len(), 1);
    assert_eq!(json["jobs"][0]["dialect"], "escpos");
    assert_eq!(json["jobs"][0]["id"], 1);
}

#[tokio::test]
async fn a_single_job_can_be_fetched_with_its_decoded_elements_and_hex_dump() {
    let store = JobStore::default();
    store.record(&EscPosEncoder.encode(&receipt()).expect("encode"));

    let app = virtual_printer::build_app(store);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/jobs/1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(json["text"].as_str().expect("text").contains("APEX STORE"));
    assert!(json["hex"].as_str().expect("hex").starts_with("1B 40"));
    assert!(!json["elements"].as_array().expect("elements").is_empty());
}

#[tokio::test]
async fn asking_for_a_job_that_never_arrived_is_a_404() {
    let app = virtual_printer::build_app(JobStore::default());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/jobs/42")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_browser_page_is_served_at_the_root() {
    let app = virtual_printer::build_app(JobStore::default());

    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .expect("request");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let html = String::from_utf8(body.to_vec()).expect("utf8");
    assert!(html.contains("Virtual Printer"));
    assert!(
        html.contains("toDataURL('image/png')"),
        "the page must be able to save the receipt as a PNG"
    );
}

#[tokio::test]
async fn a_receipt_printed_over_tcp_9100_arrives_decoded() {
    // The whole loop a stranger runs: the hub's real TCP transport on one side, the
    // virtual printer's real socket on the other, and a readable receipt at the end.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let store = JobStore::default();
    tokio::spawn(virtual_printer::socket::serve(listener, store.clone()));

    let printer = TransportPrinter::new(EscPosEncoder, Tcp9100Transport::new(addr.to_string()));
    tokio::task::spawn_blocking(move || printer.print(&receipt()))
        .await
        .expect("join")
        .expect("print");

    let job = wait_for_job(&store).await;
    assert_eq!(job.dialect, Dialect::EscPos);
    assert!(job.text.contains("APEX STORE"));
    assert!(job.warnings.is_empty(), "unexpected {:?}", job.warnings);
}

#[tokio::test]
async fn a_probe_that_sends_nothing_does_not_become_a_receipt() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let store = JobStore::default();
    tokio::spawn(virtual_printer::socket::serve(listener, store.clone()));

    drop(
        tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect and hang up"),
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    assert!(store.jobs().is_empty());
}

async fn wait_for_job(store: &JobStore) -> virtual_printer::Job {
    for _ in 0..50 {
        if let Some(job) = store.jobs().into_iter().next() {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no job arrived");
}

#[test]
fn the_store_keeps_the_most_recent_jobs_and_drops_the_oldest() {
    // A demo left running for an afternoon must not grow without bound.
    let store = JobStore::with_capacity(2);
    for _ in 0..3 {
        store.record(&EscPosEncoder.encode(&receipt()).expect("encode"));
    }

    let jobs = store.jobs();
    assert_eq!(jobs.len(), 2);
    assert_eq!(
        jobs[0].id, 2,
        "the oldest job is dropped, ids keep counting"
    );
    assert_eq!(jobs[1].id, 3);
}
