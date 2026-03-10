use apex_edge_printing::{generate_document, render};
use apex_edge_storage::{get_document, run_migrations};
use sqlx::sqlite::SqlitePoolOptions;
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
