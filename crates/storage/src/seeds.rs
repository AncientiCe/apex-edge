//! Demo data seeding for local POS frontend usage.

use apex_edge_contracts::{PromoAction, PromoCondition, Promotion, PromotionType};
use chrono::{Duration, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    insert_catalog_item, insert_category, insert_customer, insert_price_book_entry,
    insert_promotion, insert_tax_rule, PoolError,
};

#[derive(Debug, Clone, Copy)]
pub struct DemoSeedSummary {
    pub categories: usize,
    pub products: usize,
    pub customers: usize,
    pub promotions: usize,
}

/// Seed deterministic demo data for one store.
///
/// - Clears existing store-scoped catalog/customers/categories/price/tax rows
/// - Inserts enough data for catalog pagination + customer lookup
pub async fn seed_demo_data(
    pool: &SqlitePool,
    store_id: Uuid,
) -> Result<DemoSeedSummary, PoolError> {
    sqlx::query("DELETE FROM price_book_entries WHERE store_id = ?")
        .bind(store_id.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM catalog_items WHERE store_id = ?")
        .bind(store_id.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM customers WHERE store_id = ?")
        .bind(store_id.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM categories WHERE store_id = ?")
        .bind(store_id.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM tax_rules WHERE store_id = ?")
        .bind(store_id.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM promotions WHERE store_id = ?")
        .bind(store_id.to_string())
        .execute(pool)
        .await?;

    let category_names = [
        "Beverages",
        "Bakery",
        "Produce",
        "Dairy",
        "Frozen",
        "Household",
        "Snacks",
        "Personal Care",
    ];
    let tax_category_id = Uuid::from_u128(0xAAA0);
    insert_tax_rule(
        pool,
        Uuid::from_u128(0xAAA1),
        store_id,
        tax_category_id,
        700,
        "VAT 7%",
        false,
    )
    .await?;

    let mut categories = Vec::with_capacity(category_names.len());
    for (idx, name) in category_names.iter().enumerate() {
        let id = Uuid::from_u128(0x1000 + idx as u128);
        insert_category(pool, id, store_id, name).await?;
        categories.push((id, *name));
    }

    let mut product_count = 0usize;
    for p in 0..180 {
        let (category_id, category_name) = categories[p % categories.len()];
        let item_id = Uuid::from_u128(0x2000 + p as u128);
        let sku = format!("SKU{:05}", p + 1);
        let name = format!("{category_name} Item {}", p + 1);
        insert_catalog_item(
            pool,
            item_id,
            store_id,
            &sku,
            &name,
            category_id,
            tax_category_id,
        )
        .await?;
        let description = format!(
            "{} product {}, suitable for local POS demo browsing.",
            category_name,
            p + 1
        );
        sqlx::query("UPDATE catalog_items SET description = ? WHERE id = ?")
            .bind(description)
            .bind(item_id.to_string())
            .execute(pool)
            .await?;
        let price_cents = 199 + ((p as u64 % 17) * 37);
        insert_price_book_entry(pool, store_id, item_id, None, price_cents, "USD").await?;
        product_count += 1;
    }

    let mut customer_count = 0usize;
    for c in 0..120 {
        let id = Uuid::from_u128(0x3000 + c as u128);
        let code = format!("CUST{:04}", c + 1);
        let name = format!("Customer {}", c + 1);
        let email = format!("customer{:04}@demo.local", c + 1);
        insert_customer(pool, id, store_id, &code, &name).await?;
        sqlx::query("UPDATE customers SET email = ? WHERE id = ?")
            .bind(email)
            .bind(id.to_string())
            .execute(pool)
            .await?;
        customer_count += 1;
    }

    // Promotions: mix of coupons (code-based) and automatic (config-based), as if pulled from HQ.
    let now = Utc::now();
    let valid_from = now - Duration::days(1);
    let valid_until = Some(now + Duration::days(90));

    // Coupon: 10% off basket
    let promo_save10 = Promotion {
        id: Uuid::from_u128(0x4001),
        code: Some("SAVE10".into()),
        name: "10% off basket".into(),
        promo_type: PromotionType::PercentageOff { percent_bps: 1000 },
        priority: 10,
        valid_from,
        valid_until,
        conditions: vec![],
        actions: vec![PromoAction::ApplyToBasket],
        version: 1,
    };
    insert_promotion(
        pool,
        promo_save10.id,
        store_id,
        &serde_json::to_string(&promo_save10).unwrap_or_default(),
    )
    .await?;

    // Coupon: $5 off basket
    let promo_flat5 = Promotion {
        id: Uuid::from_u128(0x4002),
        code: Some("FLAT5".into()),
        name: "$5 off".into(),
        promo_type: PromotionType::FixedAmountOff { amount_cents: 500 },
        priority: 5,
        valid_from,
        valid_until,
        conditions: vec![],
        actions: vec![PromoAction::ApplyToBasket],
        version: 1,
    };
    insert_promotion(
        pool,
        promo_flat5.id,
        store_id,
        &serde_json::to_string(&promo_flat5).unwrap_or_default(),
    )
    .await?;

    // Coupon: 20% off (VIP)
    let promo_vip20 = Promotion {
        id: Uuid::from_u128(0x4003),
        code: Some("VIP20".into()),
        name: "VIP 20% off".into(),
        promo_type: PromotionType::PercentageOff { percent_bps: 2000 },
        priority: 20,
        valid_from,
        valid_until,
        conditions: vec![],
        actions: vec![PromoAction::ApplyToBasket],
        version: 1,
    };
    insert_promotion(
        pool,
        promo_vip20.id,
        store_id,
        &serde_json::to_string(&promo_vip20).unwrap_or_default(),
    )
    .await?;

    // Automatic: min basket 2000 cents ($20), 5% off basket
    let promo_auto_min = Promotion {
        id: Uuid::from_u128(0x4004),
        code: None,
        name: "Spend $20 get 5% off".into(),
        promo_type: PromotionType::PercentageOff { percent_bps: 500 },
        priority: 8,
        valid_from,
        valid_until,
        conditions: vec![PromoCondition::MinBasketAmount { amount_cents: 2000 }],
        actions: vec![PromoAction::ApplyToBasket],
        version: 1,
    };
    insert_promotion(
        pool,
        promo_auto_min.id,
        store_id,
        &serde_json::to_string(&promo_auto_min).unwrap_or_default(),
    )
    .await?;

    // Automatic: Beverages category 10% off (category_id = 0x1000)
    let beverages_id = categories[0].0;
    let promo_beverages = Promotion {
        id: Uuid::from_u128(0x4005),
        code: None,
        name: "Beverages 10% off".into(),
        promo_type: PromotionType::PercentageOff { percent_bps: 1000 },
        priority: 7,
        valid_from,
        valid_until,
        conditions: vec![],
        actions: vec![PromoAction::ApplyToCategory {
            category_id: beverages_id,
            max_quantity: None,
        }],
        version: 1,
    };
    insert_promotion(
        pool,
        promo_beverages.id,
        store_id,
        &serde_json::to_string(&promo_beverages).unwrap_or_default(),
    )
    .await?;

    // Automatic: Buy 2 of Bakery Item 106, get 50% off each (only the first 2 units)
    let bakery_item_106_id = Uuid::from_u128(0x2000 + 105);
    let promo_bakery_106 = Promotion {
        id: Uuid::from_u128(0x4006),
        code: None,
        name: "Buy 2 of Bakery Item 106 get 50% off each".into(),
        promo_type: PromotionType::PercentageOff { percent_bps: 5000 },
        priority: 9,
        valid_from,
        valid_until,
        conditions: vec![PromoCondition::ItemInBasket {
            item_id: bakery_item_106_id,
            min_quantity: 2,
        }],
        actions: vec![PromoAction::ApplyToItem {
            item_id: bakery_item_106_id,
            max_quantity: Some(2),
        }],
        version: 1,
    };
    insert_promotion(
        pool,
        promo_bakery_106.id,
        store_id,
        &serde_json::to_string(&promo_bakery_106).unwrap_or_default(),
    )
    .await?;

    let promotion_count = 6usize;

    Ok(DemoSeedSummary {
        categories: categories.len(),
        products: product_count,
        customers: customer_count,
        promotions: promotion_count,
    })
}
