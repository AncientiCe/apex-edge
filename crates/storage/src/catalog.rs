//! Catalog and reference data (ingested by sync or seeded for tests).

use apex_edge_contracts::PriceBookEntry;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

#[derive(Debug, Clone)]
pub struct CatalogItemRow {
    pub id: Uuid,
    pub store_id: Uuid,
    pub sku: String,
    pub name: String,
    pub category_id: Uuid,
    pub tax_category_id: Uuid,
}

pub async fn get_catalog_item(
    pool: &SqlitePool,
    store_id: Uuid,
    item_id: Uuid,
) -> Result<Option<CatalogItemRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, String)>(
        "SELECT id, store_id, sku, name, category_id, tax_category_id FROM catalog_items WHERE id = ? AND store_id = ?",
    )
    .bind(item_id.to_string())
    .bind(store_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(id, store_id, sku, name, category_id, tax_category_id)| CatalogItemRow {
            id: Uuid::parse_str(&id).unwrap_or_default(),
            store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
            sku,
            name,
            category_id: Uuid::parse_str(&category_id).unwrap_or_default(),
            tax_category_id: Uuid::parse_str(&tax_category_id).unwrap_or_default(),
        },
    ))
}

pub async fn get_catalog_item_by_sku(
    pool: &SqlitePool,
    store_id: Uuid,
    sku: &str,
) -> Result<Option<CatalogItemRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, String)>(
        "SELECT id, store_id, sku, name, category_id, tax_category_id FROM catalog_items WHERE store_id = ? AND sku = ?",
    )
    .bind(store_id.to_string())
    .bind(sku)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(id, store_id, sku, name, category_id, tax_category_id)| CatalogItemRow {
            id: Uuid::parse_str(&id).unwrap_or_default(),
            store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
            sku,
            name,
            category_id: Uuid::parse_str(&category_id).unwrap_or_default(),
            tax_category_id: Uuid::parse_str(&tax_category_id).unwrap_or_default(),
        },
    ))
}

pub async fn insert_catalog_item(
    pool: &SqlitePool,
    id: Uuid,
    store_id: Uuid,
    sku: &str,
    name: &str,
    category_id: Uuid,
    tax_category_id: Uuid,
) -> Result<(), PoolError> {
    sqlx::query(
        "INSERT OR REPLACE INTO catalog_items (id, store_id, sku, name, category_id, tax_category_id) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(sku)
    .bind(name)
    .bind(category_id.to_string())
    .bind(tax_category_id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_price_book_entries(
    pool: &SqlitePool,
    store_id: Uuid,
) -> Result<Vec<PriceBookEntry>, PoolError> {
    let rows = sqlx::query_as::<_, (String, Option<String>, i64, String)>(
        "SELECT item_id, modifier_option_id, price_cents, currency FROM price_book_entries WHERE store_id = ?",
    )
    .bind(store_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(item_id, modifier_option_id, price_cents, currency)| PriceBookEntry {
                item_id: Uuid::parse_str(&item_id).unwrap_or_default(),
                modifier_option_id: modifier_option_id.and_then(|s| Uuid::parse_str(&s).ok()),
                price_cents: price_cents as u64,
                currency,
            },
        )
        .collect())
}

pub async fn insert_price_book_entry(
    pool: &SqlitePool,
    store_id: Uuid,
    item_id: Uuid,
    modifier_option_id: Option<Uuid>,
    price_cents: u64,
    currency: &str,
) -> Result<(), PoolError> {
    sqlx::query(
        "INSERT INTO price_book_entries (store_id, item_id, modifier_option_id, price_cents, currency) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(store_id.to_string())
    .bind(item_id.to_string())
    .bind(modifier_option_id.map(|u| u.to_string()))
    .bind(price_cents as i64)
    .bind(currency)
    .execute(pool)
    .await?;
    Ok(())
}
