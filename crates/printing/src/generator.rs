//! Generate documents (receipt, kitchen chit, etc.) and persist for retrieval.

use apex_edge_storage::documents::{enqueue_document, mark_failed, mark_generated};
use apex_edge_storage::PoolError;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::render::render;

/// Generate a document and store its content for later retrieval by the POS.
///
/// `template_body` is provided by configuration sync (or another local source).
/// `payload_json` is the contracted data used to fill the template.
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

    let payload: serde_json::Value =
        serde_json::from_str(payload_json).unwrap_or(serde_json::Value::Null);
    match render(template_body, &payload) {
        Ok(bytes) => {
            let content = String::from_utf8_lossy(&bytes).to_string();
            mark_generated(pool, document_id, mime_type, &content).await?;
            Ok(())
        }
        Err(e) => {
            mark_failed(pool, document_id, &e.to_string()).await?;
            Ok(())
        }
    }
}
