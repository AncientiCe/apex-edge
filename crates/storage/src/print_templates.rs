//! Print templates synced from HQ; lookup by store and document type.

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone)]
pub struct PrintTemplateRow {
    pub store_id: Uuid,
    pub document_type: String,
    pub template_id: Uuid,
    pub template_body: String,
    pub version: i64,
}

/// Upsert a print template for (store_id, document_type). Replaces existing row.
pub async fn upsert_print_template(
    pool: &SqlitePool,
    store_id: Uuid,
    document_type: &str,
    template_id: Uuid,
    template_body: &str,
    version: i64,
) -> Result<(), PoolError> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO print_templates (store_id, document_type, template_id, template_body, version, created_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(store_id, document_type) DO UPDATE SET \
         template_id = excluded.template_id, template_body = excluded.template_body, version = excluded.version, created_at = excluded.created_at",
    )
    .bind(store_id.to_string())
    .bind(document_type)
    .bind(template_id.to_string())
    .bind(template_body)
    .bind(version)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get the active print template for a store and document type, if any.
pub async fn get_print_template(
    pool: &SqlitePool,
    store_id: Uuid,
    document_type: &str,
) -> Result<Option<PrintTemplateRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String, i64)>(
        "SELECT store_id, document_type, template_id, template_body, version FROM print_templates WHERE store_id = ? AND document_type = ?",
    )
    .bind(store_id.to_string())
    .bind(document_type)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(store_id_s, document_type_s, template_id_s, template_body, version)| PrintTemplateRow {
            store_id: Uuid::parse_str(&store_id_s).unwrap_or(Uuid::nil()),
            document_type: document_type_s,
            template_id: Uuid::parse_str(&template_id_s).unwrap_or(Uuid::nil()),
            template_body,
            version,
        },
    ))
}
