//! Catalog and reference data (ingested by sync or seeded for tests).

use apex_edge_contracts::{CatalogItem, InventoryLevel, PriceBookEntry};
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
    /// Persisted from `CatalogItem.is_active`; false = item is not sold.
    pub is_active: bool,
    /// Units available to sell; `None` = inventory not tracked (no constraint on add-to-cart qty).
    pub available_qty: Option<i64>,
    /// Explicit sellability flag from the inventory level. `None` = not yet synced.
    pub is_available: Option<bool>,
    /// Ordered product image URLs for PDP gallery.
    pub image_urls: Vec<String>,
}

type CatalogRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

fn map_catalog_row(row: CatalogRow) -> CatalogItemRow {
    let (
        id,
        store_id,
        sku,
        name,
        category_id,
        tax_category_id,
        description,
        is_active_int,
        available_qty,
        is_available_int,
        image_urls_json,
    ) = row;
    let image_urls: Vec<String> = image_urls_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    CatalogItemRow {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        store_id: Uuid::parse_str(&store_id).unwrap_or_default(),
        sku,
        name,
        category_id: Uuid::parse_str(&category_id).unwrap_or_default(),
        tax_category_id: Uuid::parse_str(&tax_category_id).unwrap_or_default(),
        description,
        is_active: is_active_int.unwrap_or(1) != 0,
        available_qty,
        is_available: is_available_int.map(|v| v != 0),
        image_urls,
    }
}

impl CatalogItemRow {
    /// Check if the requested quantity can be sold. Returns `None` if ok, or an error
    /// code string matching the POS error codes.
    ///
    /// - `is_active = false` → `OUT_OF_STOCK`
    /// - inventory tracked and `available_qty <= 0` → `OUT_OF_STOCK`
    /// - inventory tracked and `quantity > available_qty` → `INSUFFICIENT_STOCK`
    /// - inventory not tracked → `None` (allowed)
    pub fn check_quantity(&self, quantity: i64) -> Option<&'static str> {
        if !self.is_active {
            return Some("OUT_OF_STOCK");
        }
        if let Some(qty) = self.available_qty {
            if qty <= 0 {
                return Some("OUT_OF_STOCK");
            }
            if quantity > qty {
                return Some("INSUFFICIENT_STOCK");
            }
        }
        None
    }
}

const SELECT_CATALOG_COLS: &str =
    "id, store_id, sku, name, category_id, tax_category_id, description, is_active, available_qty, is_available, image_urls";

pub async fn get_catalog_item(
    pool: &SqlitePool,
    store_id: Uuid,
    item_id: Uuid,
) -> Result<Option<CatalogItemRow>, PoolError> {
    let row = sqlx::query_as::<_, CatalogRow>(&format!(
        "SELECT {SELECT_CATALOG_COLS} FROM catalog_items WHERE id = ? AND store_id = ?"
    ))
    .bind(item_id.to_string())
    .bind(store_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(map_catalog_row))
}

pub async fn get_catalog_item_by_sku(
    pool: &SqlitePool,
    store_id: Uuid,
    sku: &str,
) -> Result<Option<CatalogItemRow>, PoolError> {
    let row = sqlx::query_as::<_, CatalogRow>(&format!(
        "SELECT {SELECT_CATALOG_COLS} FROM catalog_items WHERE store_id = ? AND sku = ?"
    ))
    .bind(store_id.to_string())
    .bind(sku)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(map_catalog_row))
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
    let list_sql_base =
        format!("SELECT {SELECT_CATALOG_COLS} FROM catalog_items WHERE store_id = ?");
    let list_sql_cat = format!(
        "SELECT {SELECT_CATALOG_COLS} FROM catalog_items WHERE store_id = ? AND category_id = ?"
    );
    let list_sql_q = format!(
        "SELECT {SELECT_CATALOG_COLS} FROM catalog_items WHERE store_id = ? AND (sku = ? OR name LIKE ? OR description LIKE ?)"
    );
    let list_sql_cat_q = format!(
        "SELECT {SELECT_CATALOG_COLS} FROM catalog_items WHERE store_id = ? AND category_id = ? AND (sku = ? OR name LIKE ? OR description LIKE ?)"
    );
    let rows: Vec<CatalogRow> = match (category_id, q_trimmed, name_pattern.as_ref()) {
        (None, None, _) => {
            sqlx::query_as::<_, CatalogRow>(&format!(
                "{list_sql_base} ORDER BY name LIMIT ? OFFSET ?"
            ))
            .bind(store_id.to_string())
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await?
        }
        (Some(cid), None, _) => {
            sqlx::query_as::<_, CatalogRow>(&format!(
                "{list_sql_cat} ORDER BY name LIMIT ? OFFSET ?"
            ))
            .bind(store_id.to_string())
            .bind(cid.to_string())
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await?
        }
        (None, Some(qs), Some(pat)) => {
            sqlx::query_as::<_, CatalogRow>(&format!("{list_sql_q} ORDER BY name LIMIT ? OFFSET ?"))
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
            sqlx::query_as::<_, CatalogRow>(&format!(
                "{list_sql_cat_q} ORDER BY name LIMIT ? OFFSET ?"
            ))
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
    let items = rows.into_iter().map(map_catalog_row).collect();
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
            "INSERT INTO catalog_items (id, store_id, sku, name, category_id, tax_category_id, description, is_active) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(item.id.to_string())
        .bind(store_id.to_string())
        .bind(item.sku.as_str())
        .bind(item.name.as_str())
        .bind(item.category_id.to_string())
        .bind(item.tax_category_id.to_string())
        .bind(item.description.as_deref())
        .bind(item.is_active as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Apply a batch of inventory levels for a store.
/// Updates `available_qty`, `is_available` (stored as `available_qty` ≥ 0 or explicit flag),
/// and `image_urls` on matching `catalog_items` rows. Rows without a matching catalog item
/// are silently skipped (forward-compatibility: item may not have synced yet).
pub async fn replace_inventory_levels(
    pool: &SqlitePool,
    store_id: Uuid,
    levels: &[InventoryLevel],
) -> Result<(), PoolError> {
    let mut tx = pool.begin().await?;
    for level in levels {
        let image_urls_json =
            serde_json::to_string(&level.image_urls).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "UPDATE catalog_items SET available_qty = ?, is_available = ?, image_urls = ? WHERE id = ? AND store_id = ?",
        )
        .bind(level.available_qty)
        .bind(level.is_available as i64)
        .bind(&image_urls_json)
        .bind(level.item_id.to_string())
        .bind(store_id.to_string())
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
