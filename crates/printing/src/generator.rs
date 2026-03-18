//! Generate documents (receipt, kitchen chit, etc.) and persist for retrieval.

// Metric names (must match apex_edge_metrics::schema for consistency).
const METRIC_DOCUMENT_RENDER_TOTAL: &str = "apex_edge_document_render_total";
const METRIC_DOCUMENT_RENDER_DURATION_SECONDS: &str = "apex_edge_document_render_duration_seconds";
const OUTCOME_TEMPLATE_ERROR: &str = "template_error";
const OUTCOME_PDF_ERROR: &str = "pdf_error";
use apex_edge_storage::documents::{enqueue_document, mark_failed, mark_generated};
use apex_edge_storage::PoolError;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sqlx::SqlitePool;
use std::sync::OnceLock;
use std::time::Instant;
use uuid::Uuid;

use crate::pdf::html_to_pdf;
use crate::render::{render, render_html};

fn finalize_timing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("APEX_EDGE_PROFILE_FINALIZE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

fn log_finalize_timing(event: &str, fields: &[(&str, String)]) {
    if !finalize_timing_enabled() {
        return;
    }
    let suffix = if fields.is_empty() {
        String::new()
    } else {
        format!(
            " {}",
            fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    eprintln!("[ApexEdge][Printing] {event}{suffix}");
}

/// Bounded document_type label for metrics (no raw IDs).
fn document_type_label(document_type: &str) -> &'static str {
    match document_type {
        "customer_receipt" => "customer_receipt",
        "gift_receipt" => "gift_receipt",
        "receipt" => "receipt",
        _ => "other",
    }
}

/// Generate a document and store its content for later retrieval by the POS.
///
/// `template_body` is provided by configuration sync (or another local source).
/// `payload_json` is the contracted data used to fill the template.
/// When `mime_type` is `application/pdf`, the template is rendered as HTML and converted to PDF.
#[allow(clippy::too_many_arguments)]
pub async fn generate_document(
    pool: &SqlitePool,
    document_id: Uuid,
    document_type: &str,
    order_id: Option<Uuid>,
    cart_id: Option<Uuid>,
    template_id: Uuid,
    template_body: &str,
    payload_json: &str,
    mime_type: &str,
) -> Result<(), PoolError> {
    let started_at = Instant::now();
    enqueue_document(
        pool,
        document_id,
        document_type,
        order_id,
        cart_id,
        template_id,
        payload_json,
    )
    .await?;
    log_finalize_timing(
        "enqueue_done",
        &[
            ("document_id", document_id.to_string()),
            ("document_type", document_type.to_string()),
            ("elapsed_ms", started_at.elapsed().as_millis().to_string()),
        ],
    );

    let payload: serde_json::Value =
        serde_json::from_str(payload_json).unwrap_or(serde_json::Value::Null);

    let start = Instant::now();
    let (content, mime_used) = if mime_type == "application/pdf" {
        let render_html_started_at = Instant::now();
        match render_html(template_body, &payload) {
            Ok(html) => match html_to_pdf(&html) {
                Ok(pdf_bytes) => {
                    log_finalize_timing(
                        "render_html_done",
                        &[(
                            "elapsed_ms",
                            render_html_started_at.elapsed().as_millis().to_string(),
                        )],
                    );
                    metrics::counter!(
                        METRIC_DOCUMENT_RENDER_TOTAL,
                        1u64,
                        "document_type" => document_type_label(document_type),
                        "outcome" => "ok"
                    );
                    let content = BASE64.encode(&pdf_bytes);
                    (content, "application/pdf")
                }
                Err(e) => {
                    metrics::counter!(
                        METRIC_DOCUMENT_RENDER_TOTAL,
                        1u64,
                        "document_type" => document_type_label(document_type),
                        "outcome" => OUTCOME_PDF_ERROR
                    );
                    mark_failed(pool, document_id, &e.to_string()).await?;
                    return Ok(());
                }
            },
            Err(e) => {
                metrics::counter!(
                    METRIC_DOCUMENT_RENDER_TOTAL,
                    1u64,
                    "document_type" => document_type_label(document_type),
                    "outcome" => OUTCOME_TEMPLATE_ERROR
                );
                mark_failed(pool, document_id, &e.to_string()).await?;
                return Ok(());
            }
        }
    } else {
        match render(template_body, &payload) {
            Ok(bytes) => {
                metrics::counter!(
                    METRIC_DOCUMENT_RENDER_TOTAL,
                    1u64,
                    "document_type" => document_type_label(document_type),
                    "outcome" => "ok"
                );
                let content = String::from_utf8_lossy(&bytes).to_string();
                (content, mime_type)
            }
            Err(e) => {
                metrics::counter!(
                    METRIC_DOCUMENT_RENDER_TOTAL,
                    1u64,
                    "document_type" => document_type_label(document_type),
                    "outcome" => OUTCOME_TEMPLATE_ERROR
                );
                mark_failed(pool, document_id, &e.to_string()).await?;
                return Ok(());
            }
        }
    };
    metrics::histogram!(
        METRIC_DOCUMENT_RENDER_DURATION_SECONDS,
        start.elapsed().as_secs_f64(),
        "document_type" => document_type_label(document_type)
    );
    log_finalize_timing(
        "render_pipeline_done",
        &[
            ("document_id", document_id.to_string()),
            ("elapsed_ms", start.elapsed().as_millis().to_string()),
            ("mime_used", mime_used.to_string()),
        ],
    );

    mark_generated(pool, document_id, mime_used, &content).await?;
    log_finalize_timing(
        "mark_generated_done",
        &[
            ("document_id", document_id.to_string()),
            ("elapsed_ms", started_at.elapsed().as_millis().to_string()),
        ],
    );
    Ok(())
}
