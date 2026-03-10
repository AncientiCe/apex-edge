//! Catalog and reference data (ingested by sync or seeded for tests).

use apex_edge_contracts::{CatalogItem, PriceBookEntry};
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
    pub description: Option<String>,
}

fn map_catalog_row(
    id: String,
    store_id: String,
    sku: String,
    name: String,
    category_id: String,
    tax_category_id: String,
    description: Option<String>,
) -> CatalogItemRow {
    CatalogItemRow {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
        sku,
        name,
        category_id: Uuid::parse_str(&category_id).unwrap_or_default(),
        tax_category_id: Uuid::parse_str(&tax_category_id).unwrap_or_default(),
        description,
    }
}

pub async fn get_catalog_item(
    pool: &SqlitePool,
    store_id: Uuid,
    item_id: Uuid,
) -> Result<Option<CatalogItemRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, String, Option<String>)>(
        "SELECT id, store_id, sku, name, category_id, tax_category_id, description FROM catalog_items WHERE id = ? AND store_id = ?",
    )
    .bind(item_id.to_string())
    .bind(store_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(id, store_id, sku, name, category_id, tax_category_id, description)| {
            map_catalog_row(
                id,
                store_id,
                sku,
                name,
                category_id,
                tax_category_id,
                description,
            )
        },
    ))
}

pub async fn get_catalog_item_by_sku(
    pool: &SqlitePool,
    store_id: Uuid,
    sku: &str,
) -> Result<Option<CatalogItemRow>, PoolError> {
    let row = sqlx::query_as::<_, (String, String, String, String, String, String, Option<String>)>(
        "SELECT id, store_id, sku, name, category_id, tax_category_id, description FROM catalog_items WHERE store_id = ? AND sku = ?",
    )
    .bind(store_id.to_string())
    .bind(sku)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(id, store_id, sku, name, category_id, tax_category_id, description)| {
            map_catalog_row(
                id,
                store_id,
                sku,
                name,
                category_id,
                tax_category_id,
                description,
            )
        },
    ))
}

pub async fn list_catalog_items(
    pool: &SqlitePool,
    store_id: Uuid,
    category_id: Option<Uuid>,
    q: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<(Vec<CatalogItemRow>, u64), PoolError> {
    let limit = limit.clamp(1, 100);
    let q_trimmed = q.map(|s| s.trim()).filter(|s| !s.is_empty());
    let name_pattern = q_trimmed.map(|s| format!("%{s}%"));
    let count_sql = match (category_id.is_some(), q_trimmed.is_some()) {
        (false, false) => "SELECT COUNT(*) FROM catalog_items WHERE store_id = ?",
        (true, false) => "SELECT COUNT(*) FROM catalog_items WHERE store_id = ? AND category_id = ?",
        (false, true) => "SELECT COUNT(*) FROM catalog_items WHERE store_id = ? AND (sku = ? OR name LIKE ? OR description LIKE ?)",
        (true, true) => "SELECT COUNT(*) FROM catalog_items WHERE store_id = ? AND category_id = ? AND (sku = ? OR name LIKE ? OR description LIKE ?)",
    };
    let total: (i64,) = match (category_id, q_trimmed, name_pattern.as_ref()) {
        (None, None, _) => {
            sqlx::query_as(count_sql)
                .bind(store_id.to_string())
                .fetch_one(pool)
                .await?
        }
        (Some(cid), None, _) => {
            sqlx::query_as(count_sql)
                .bind(store_id.to_string())
                .bind(cid.to_string())
                .fetch_one(pool)
                .await?
        }
        (None, Some(qs), Some(pat)) => {
            sqlx::query_as(count_sql)
                .bind(store_id.to_string())
                .bind(qs)
                .bind(pat)
                .bind(pat)
                .fetch_one(pool)
                .await?
        }
        (Some(cid), Some(qs), Some(pat)) => {
            sqlx::query_as(count_sql)
                .bind(store_id.to_string())
                .bind(cid.to_string())
                .bind(qs)
                .bind(pat)
                .bind(pat)
                .fetch_one(pool)
                .await?
        }
        _ => (0,),
    };
    let total = total.0 as u64;
    let list_sql = match (category_id.is_some(), q_trimmed.is_some()) {
        (false, false) => "SELECT id, store_id, sku, name, category_id, tax_category_id, description FROM catalog_items WHERE store_id = ? ORDER BY name LIMIT ? OFFSET ?",
        (true, false) => "SELECT id, store_id, sku, name, category_id, tax_category_id, description FROM catalog_items WHERE store_id = ? AND category_id = ? ORDER BY name LIMIT ? OFFSET ?",
        (false, true) => "SELECT id, store_id, sku, name, category_id, tax_category_id, description FROM catalog_items WHERE store_id = ? AND (sku = ? OR name LIKE ? OR description LIKE ?) ORDER BY name LIMIT ? OFFSET ?",
        (true, true) => "SELECT id, store_id, sku, name, category_id, tax_category_id, description FROM catalog_items WHERE store_id = ? AND category_id = ? AND (sku = ? OR name LIKE ? OR description LIKE ?) ORDER BY name LIMIT ? OFFSET ?",
    };
    let rows = match (category_id, q_trimmed, name_pattern.as_ref()) {
        (None, None, _) => {
            sqlx::query_as::<
                _,
                (
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    Option<String>,
                ),
            >(list_sql)
            .bind(store_id.to_string())
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await?
        }
        (Some(cid), None, _) => {
            sqlx::query_as::<
                _,
                (
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    Option<String>,
                ),
            >(list_sql)
            .bind(store_id.to_string())
            .bind(cid.to_string())
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await?
        }
        (None, Some(qs), Some(pat)) => {
            sqlx::query_as::<
                _,
                (
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    Option<String>,
                ),
            >(list_sql)
            .bind(store_id.to_string())
            .bind(qs)
            .bind(pat)
            .bind(pat)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await?
        }
        (Some(cid), Some(qs), Some(pat)) => {
            sqlx::query_as::<
                _,
                (
                    String,
                    String,
                    String,
                    String,
                    String,
                    String,
                    Option<String>,
                ),
            >(list_sql)
            .bind(store_id.to_string())
            .bind(cid.to_string())
            .bind(qs)
            .bind(pat)
            .bind(pat)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await?
        }
        _ => vec![],
    };
    let items = rows
        .into_iter()
        .map(
            |(id, store_id, sku, name, category_id, tax_category_id, description)| {
                map_catalog_row(
                    id,
                    store_id,
                    sku,
                    name,
                    category_id,
                    tax_category_id,
                    description,
                )
            },
        )
        .collect();
    Ok((items, total))
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
        "INSERT OR REPLACE INTO catalog_items (id, store_id, sku, name, category_id, tax_category_id, description) VALUES (?, ?, ?, ?, ?, ?, NULL)",
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

/// Replace all catalog items for a store atomically (delete + insert in one transaction).
/// Used during sync to apply a full snapshot of catalog data.
pub async fn replace_catalog_items(
    pool: &SqlitePool,
    store_id: Uuid,
    items: &[CatalogItem],
) -> Result<(), PoolError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM catalog_items WHERE store_id = ?")
        .bind(store_id.to_string())
        .execute(&mut *tx)
        .await?;
    for item in items {
        sqlx::query(
            "INSERT INTO catalog_items (id, store_id, sku, name, category_id, tax_category_id, description) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(item.id.to_string())
        .bind(store_id.to_string())
        .bind(item.sku.as_str())
        .bind(item.name.as_str())
        .bind(item.category_id.to_string())
        .bind(item.tax_category_id.to_string())
        .bind(item.description.as_deref())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Update the description of an existing catalog item. Used after insert when a description
/// is available (e.g. during sync where `insert_catalog_item` does not accept a description
/// to avoid exceeding the argument count lint threshold).
pub async fn update_catalog_item_description(
    pool: &SqlitePool,
    id: Uuid,
    description: &str,
) -> Result<(), PoolError> {
    sqlx::query("UPDATE catalog_items SET description = ? WHERE id = ?")
        .bind(description)
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

/// Replace all price book entries for a store atomically (delete + insert in one transaction).
/// Used during sync to apply a full snapshot of price book data.
pub async fn replace_price_book_entries(
    pool: &SqlitePool,
    store_id: Uuid,
    entries: &[(Uuid, Option<Uuid>, u64, String)],
) -> Result<(), PoolError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM price_book_entries WHERE store_id = ?")
        .bind(store_id.to_string())
        .execute(&mut *tx)
        .await?;
    for (item_id, modifier_option_id, price_cents, currency) in entries {
        sqlx::query(
            "INSERT INTO price_book_entries (store_id, item_id, modifier_option_id, price_cents, currency) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(store_id.to_string())
        .bind(item_id.to_string())
        .bind(modifier_option_id.map(|u| u.to_string()))
        .bind(*price_cents as i64)
        .bind(currency.as_str())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
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
