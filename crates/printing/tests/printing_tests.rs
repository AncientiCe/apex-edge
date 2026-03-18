use apex_edge_printing::{generate_document, render};
use apex_edge_storage::{get_document, run_migrations};
use base64::Engine;
use sqlx::sqlite::SqlitePoolOptions;
use std::time::Instant;
use uuid::Uuid;

#[test]
fn render_replaces_placeholders() {
    let out = render("Hello {{name}}", &serde_json::json!({"name": "Apex"})).expect("render");
    assert_eq!(String::from_utf8_lossy(&out), "Hello Apex");
}

#[tokio::test]
async fn generate_document_persists_generated_content() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let doc_id = Uuid::new_v4();
    generate_document(
        &pool,
        doc_id,
        "receipt",
        Some(Uuid::new_v4()),
        None,
        Uuid::new_v4(),
        "Receipt {{total}}",
        r#"{"total":"12.34"}"#,
        "text/plain",
    )
    .await
    .expect("generate");

    let doc = get_document(&pool, doc_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(doc.status, "generated");
    assert_eq!(doc.content.as_deref(), Some("Receipt 12.34"));
}

#[tokio::test]
async fn generate_pdf_document_is_fast_and_valid_pdf() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let doc_id = Uuid::new_v4();
    let started_at = Instant::now();
    generate_document(
        &pool,
        doc_id,
        "customer_receipt",
        Some(Uuid::new_v4()),
        None,
        Uuid::new_v4(),
        "<html><body>Receipt {{total}}</body></html>",
        r#"{"total":"12.34"}"#,
        "application/pdf",
    )
    .await
    .expect("generate");
    let elapsed_ms = started_at.elapsed().as_millis();

    let doc = get_document(&pool, doc_id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(doc.status, "generated");
    assert_eq!(doc.mime_type, "application/pdf");
    let encoded = doc.content.expect("pdf content");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .expect("valid base64");
    assert!(
        bytes.starts_with(b"%PDF-"),
        "decoded output must be a PDF document"
    );
    assert!(
        elapsed_ms < 1000,
        "pdf generation should be local and fast in this path; took {elapsed_ms}ms"
    );
}

#[tokio::test]
async fn generate_pdf_document_respects_basic_html_line_breaks() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let doc_id = Uuid::new_v4();
    generate_document(
        &pool,
        doc_id,
        "customer_receipt",
        Some(Uuid::new_v4()),
        None,
        Uuid::new_v4(),
        "<html><body><p>Header</p><div>Line 1<br/>Line 2</div></body></html>",
        "{}",
        "application/pdf",
    )
    .await
    .expect("generate");

    let doc = get_document(&pool, doc_id)
        .await
        .expect("get")
        .expect("exists");
    let encoded = doc.content.expect("pdf content");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .expect("valid base64");
    let pdf_text = String::from_utf8_lossy(&bytes);
    assert!(
        pdf_text.contains("(Header) Tj"),
        "header line must be present"
    );
    assert!(
        pdf_text.contains("T*\n(Line 1) Tj"),
        "line 1 must be on its own row"
    );
    assert!(
        pdf_text.contains("T*\n(Line 2) Tj"),
        "line 2 must be on its own row"
    );
}
