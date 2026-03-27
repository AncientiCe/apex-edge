//! Coupon definitions (HQ -> ApexEdge sync).

use apex_edge_contracts::CouponDefinition;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::pool::PoolError;

fn parse_datetime(s: &str) -> DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

pub async fn upsert_coupon_definition(
    pool: &SqlitePool,
    store_id: Uuid,
    coupon: &CouponDefinition,
) -> Result<(), PoolError> {
    sqlx::query(
        "INSERT OR REPLACE INTO coupon_definitions \
         (id, store_id, code, promo_id, max_redemptions_total, max_redemptions_per_customer, valid_from, valid_until, version) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(coupon.id.to_string())
    .bind(store_id.to_string())
    .bind(&coupon.code)
    .bind(coupon.promo_id.to_string())
    .bind(coupon.max_redemptions_total.map(|v| v as i64))
    .bind(coupon.max_redemptions_per_customer.map(|v| v as i64))
    .bind(coupon.valid_from.to_rfc3339())
    .bind(coupon.valid_until.map(|v| v.to_rfc3339()))
    .bind(coupon.version as i64)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_coupon_definition_by_code(
    pool: &SqlitePool,
    store_id: Uuid,
    code: &str,
) -> Result<Option<CouponDefinition>, PoolError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<i64>,
            Option<i64>,
            String,
            Option<String>,
            i64,
        ),
    >(
        "SELECT id, code, promo_id, max_redemptions_total, max_redemptions_per_customer, valid_from, valid_until, version \
         FROM coupon_definitions WHERE store_id = ? AND LOWER(code) = LOWER(?)",
    )
    .bind(store_id.to_string())
    .bind(code)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(
            id,
            code,
            promo_id,
            max_redemptions_total,
            max_redemptions_per_customer,
            valid_from,
            valid_until,
            version,
        )| CouponDefinition {
            id: Uuid::parse_str(&id).unwrap_or_default(),
            code,
            promo_id: Uuid::parse_str(&promo_id).unwrap_or_default(),
            max_redemptions_total: max_redemptions_total.map(|v| v as u64),
            max_redemptions_per_customer: max_redemptions_per_customer.map(|v| v as u32),
            valid_from: parse_datetime(&valid_from),
            valid_until: valid_until.as_deref().map(parse_datetime),
            version: version as u64,
        },
    ))
}
