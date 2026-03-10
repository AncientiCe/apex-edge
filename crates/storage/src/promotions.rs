//! Promotions (HQ -> ApexEdge sync or seeded).

use apex_edge_contracts::Promotion;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

pub async fn list_promotions(pool: &SqlitePool, store_id: Uuid) -> Result<Vec<Promotion>, PoolError> {
    let rows = sqlx::query_as::<_, (String,)>("SELECT data FROM promotions WHERE store_id = ?")
        .bind(store_id.to_string())
        .fetch_all(pool)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (data,) in rows {
        if let Ok(p) = serde_json::from_str::<Promotion>(&data) {
            out.push(p);
        }
    }
    Ok(out)
}

pub async fn insert_promotion(
    pool: &SqlitePool,
    id: Uuid,
    store_id: Uuid,
    data: &str,
) -> Result<(), PoolError> {
    sqlx::query("INSERT OR REPLACE INTO promotions (id, store_id, data) VALUES (?, ?, ?)")
        .bind(id.to_string())
        .bind(store_id.to_string())
        .bind(data)
        .execute(pool)
        .await?;
    Ok(())
}
