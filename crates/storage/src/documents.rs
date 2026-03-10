//! Generated documents (rendered by ApexEdge, fetched by POS for printing).

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone)]
pub struct DocumentRow {
    pub id: Uuid,
    pub document_type: String,
    pub order_id: Option<Uuid>,
    pub cart_id: Option<Uuid>,
    pub status: String,
    pub template_id: Uuid,
    pub payload: String,
    pub mime_type: String,
    pub content: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
}

pub async fn enqueue_document(
    pool: &SqlitePool,
    id: Uuid,
    document_type: &str,
    order_id: Option<Uuid>,
    cart_id: Option<Uuid>,
    template_id: Uuid,
    payload: &str,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO documents (id, document_type, order_id, cart_id, status, template_id, payload, created_at) \
         VALUES (?, ?, ?, ?, 'queued', ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(document_type)
    .bind(order_id.map(|u| u.to_string()))
    .bind(cart_id.map(|u| u.to_string()))
    .bind(template_id.to_string())
    .bind(payload)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_generated(
    pool: &SqlitePool,
    id: Uuid,
    mime_type: &str,
    content: &str,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE documents SET status = 'generated', mime_type = ?, content = ?, completed_at = ? WHERE id = ?",
    )
    .bind(mime_type)
    .bind(content)
    .bind(&now)
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_failed(
    pool: &SqlitePool,
    id: Uuid,
    error_message: &str,
) -> Result<(), PoolError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE documents SET status = 'failed', completed_at = ?, error_message = ? WHERE id = ?",
    )
    .bind(&now)
    .bind(error_message)
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_document(pool: &SqlitePool, id: Uuid) -> Result<Option<DocumentRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, String, String, String, String, Option<String>, String, Option<String>, Option<String>)>(
        "SELECT id, document_type, order_id, cart_id, status, template_id, payload, mime_type, content, created_at, completed_at, error_message FROM documents WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(
            id,
            document_type,
            order_id,
            cart_id,
            status,
            template_id,
            payload,
            mime_type,
            content,
            created_at,
            completed_at,
            error_message,
        )| DocumentRow {
            id: Uuid::parse_str(&id).unwrap_or(Uuid::nil()),
            document_type,
            order_id: order_id.and_then(|s| Uuid::parse_str(&s).ok()),
            cart_id: cart_id.and_then(|s| Uuid::parse_str(&s).ok()),
            status,
            template_id: Uuid::parse_str(&template_id).unwrap_or(Uuid::nil()),
            payload,
            mime_type,
            content,
            created_at,
            completed_at,
            error_message,
        },
    ))
}

pub async fn list_documents_for_order(
    pool: &SqlitePool,
    order_id: Uuid,
) -> Result<Vec<DocumentRow>, PoolError> {
    let rows = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, String, String, String, String, Option<String>, String, Option<String>, Option<String>)>(
        "SELECT id, document_type, order_id, cart_id, status, template_id, payload, mime_type, content, created_at, completed_at, error_message FROM documents WHERE order_id = ? ORDER BY created_at",
    )
    .bind(order_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                document_type,
                order_id,
                cart_id,
                status,
                template_id,
                payload,
                mime_type,
                content,
                created_at,
                completed_at,
                error_message,
            )| DocumentRow {
                id: Uuid::parse_str(&id).unwrap_or(Uuid::nil()),
                document_type,
                order_id: order_id.and_then(|s| Uuid::parse_str(&s).ok()),
                cart_id: cart_id.and_then(|s| Uuid::parse_str(&s).ok()),
                status,
                template_id: Uuid::parse_str(&template_id).unwrap_or(Uuid::nil()),
                payload,
                mime_type,
                content,
                created_at,
                completed_at,
                error_message,
            },
        )
        .collect())
}
