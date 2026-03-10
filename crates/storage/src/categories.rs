//! Categories for catalog browsing and filtering.

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone)]
pub struct CategoryRow {
    pub id: Uuid,
    pub store_id: Uuid,
    pub name: String,
}

pub async fn list_categories(
    pool: &SqlitePool,
    store_id: Uuid,
) -> Result<Vec<CategoryRow>, PoolError> {
    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, store_id, name FROM categories WHERE store_id = ? ORDER BY name",
    )
    .bind(store_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, store_id, name)| CategoryRow {
            id: Uuid::parse_str(&id).unwrap_or_default(),
            store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
            name,
        })
        .collect())
}

pub async fn insert_category(
    pool: &SqlitePool,
    id: Uuid,
    store_id: Uuid,
    name: &str,
) -> Result<(), PoolError> {
    sqlx::query("INSERT OR REPLACE INTO categories (id, store_id, name) VALUES (?, ?, ?)")
        .bind(id.to_string())
        .bind(store_id.to_string())
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}
