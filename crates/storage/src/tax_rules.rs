//! Tax rules (HQ -> ApexEdge sync or seeded).

use apex_edge_contracts::TaxRule;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

pub async fn list_tax_rules(pool: &SqlitePool, store_id: Uuid) -> Result<Vec<TaxRule>, PoolError> {
    let rows = sqlx::query_as::<_, (String, String, i32, String, i32)>(
        "SELECT id, tax_category_id, rate_bps, name, inclusive FROM tax_rules WHERE store_id = ?",
    )
    .bind(store_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, tax_category_id, rate_bps, name, inclusive)| TaxRule {
            id: Uuid::parse_str(&id).unwrap_or_default(),
            tax_category_id: Uuid::parse_str(&tax_category_id).unwrap_or_default(),
            rate_bps: rate_bps as u32,
            name,
            inclusive: inclusive != 0,
            version: 1,
        })
        .collect())
}

pub async fn insert_tax_rule(
    pool: &SqlitePool,
    id: Uuid,
    store_id: Uuid,
    tax_category_id: Uuid,
    rate_bps: u32,
    name: &str,
    inclusive: bool,
) -> Result<(), PoolError> {
    sqlx::query(
        "INSERT OR REPLACE INTO tax_rules (id, store_id, tax_category_id, rate_bps, name, inclusive) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(store_id.to_string())
    .bind(tax_category_id.to_string())
    .bind(rate_bps as i32)
    .bind(name)
    .bind(if inclusive { 1 } else { 0 })
    .execute(pool)
    .await?;
    Ok(())
}
