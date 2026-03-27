use std::sync::Arc;

use apex_edge_contracts::{
    CatalogItem, Category, CouponDefinition, Customer, InventoryLevel, PriceBook, PriceBookEntry,
    PromoAction, PromoCondition, Promotion, PromotionType, TaxRule,
};
use axum::{extract::Path, http::StatusCode, response::IntoResponse, routing::get, Router};
use base64::Engine;
use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/sync/ndjson/:entity", get(sync_entity))
}

#[derive(Serialize)]
struct NdjsonMeta {
    total: u64,
}

async fn sync_entity(Path(entity): Path<String>) -> impl IntoResponse {
    let items = demo_catalog_items();
    let lines = match entity.as_str() {
        "catalog" => items
            .iter()
            .map(|i| b64(&serde_json::to_vec(i).unwrap_or_default()))
            .collect::<Vec<_>>(),
        "categories" => demo_categories()
            .iter()
            .map(|i| b64(&serde_json::to_vec(i).unwrap_or_default()))
            .collect::<Vec<_>>(),
        "price_book" => {
            let book = demo_price_book_for_catalog(&items);
            vec![b64(&serde_json::to_vec(&book).unwrap_or_default())]
        }
        "tax_rules" => demo_tax_rules()
            .iter()
            .map(|i| b64(&serde_json::to_vec(i).unwrap_or_default()))
            .collect::<Vec<_>>(),
        "promotions" => demo_promotions()
            .iter()
            .map(|i| b64(&serde_json::to_vec(i).unwrap_or_default()))
            .collect::<Vec<_>>(),
        "customers" => demo_customers()
            .iter()
            .map(|i| b64(&serde_json::to_vec(i).unwrap_or_default()))
            .collect::<Vec<_>>(),
        "inventory" => demo_inventory_levels_for_catalog(&items)
            .iter()
            .map(|i| b64(&serde_json::to_vec(i).unwrap_or_default()))
            .collect::<Vec<_>>(),
        "coupons" => demo_coupons()
            .iter()
            .map(|i| b64(&serde_json::to_vec(i).unwrap_or_default()))
            .collect::<Vec<_>>(),
        "print_templates" => demo_print_templates()
            .iter()
            .map(|i| b64(&serde_json::to_vec(i).unwrap_or_default()))
            .collect::<Vec<_>>(),
        _ => {
            return (
                StatusCode::NOT_FOUND,
                [("content-type", "application/json")],
                serde_json::json!({"error":"unknown_entity"}).to_string(),
            )
                .into_response();
        }
    };
    (
        StatusCode::OK,
        [("content-type", "application/x-ndjson")],
        ndjson_body(lines),
    )
        .into_response()
}

fn ndjson_body(lines: Vec<String>) -> String {
    let total = lines.len() as u64;
    let mut out = serde_json::to_string(&NdjsonMeta { total })
        .unwrap_or_else(|_| "{\"total\":0}".to_string());
    out.push('\n');
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn b64(bytes: &[u8]) -> String {
    serde_json::to_string(&base64::engine::general_purpose::STANDARD.encode(bytes))
        .unwrap_or_else(|_| "\"\"".to_string())
}

fn demo_catalog_items() -> Vec<CatalogItem> {
    let category_id = Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
    let tax_id = Uuid::parse_str("20000000-0000-0000-0000-000000000002").unwrap();
    (1..=180)
        .map(|n| CatalogItem {
            id: Uuid::from_u128(0x30000000000000000000000000000000u128 + n as u128),
            sku: format!("DEMO-{n:03}"),
            name: format!("Example Product {n}"),
            description: Some(format!("Description {n}")),
            category_id,
            tax_category_id: tax_id,
            modifiers: vec![],
            is_active: true,
            title: Some(format!("Example Product {n}")),
            brand: None,
            caption: None,
            external_identifiers: None,
            images: None,
            is_preorder: None,
            online_from: None,
            serialized_inventory: None,
            extended_attributes: None,
            variations: None,
            variation_attributes: None,
            version: 1,
        })
        .collect()
}

fn demo_categories() -> Vec<Category> {
    vec![Category {
        id: Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap(),
        name: "Demo Category".to_string(),
        parent_id: None,
        sort_order: 0,
        version: 1,
    }]
}

fn demo_price_book_for_catalog(items: &[CatalogItem]) -> PriceBook {
    PriceBook {
        id: Uuid::parse_str("40000000-0000-0000-0000-000000000001").unwrap(),
        name: "Default".to_string(),
        effective_from: Utc::now(),
        effective_until: None,
        entries: items
            .iter()
            .enumerate()
            .map(|(idx, item)| PriceBookEntry {
                item_id: item.id,
                modifier_option_id: None,
                price_cents: 199 + ((idx as u64 % 17) * 37),
                currency: "USD".to_string(),
            })
            .collect(),
        version: 1,
    }
}

fn demo_tax_rules() -> Vec<TaxRule> {
    vec![TaxRule {
        id: Uuid::parse_str("50000000-0000-0000-0000-000000000001").unwrap(),
        tax_category_id: Uuid::parse_str("20000000-0000-0000-0000-000000000002").unwrap(),
        rate_bps: 0,
        name: "No tax".to_string(),
        inclusive: false,
        version: 1,
    }]
}

fn demo_promotions() -> Vec<Promotion> {
    let item_demo_001 = Uuid::parse_str("30000000-0000-0000-0000-000000000001").unwrap();
    vec![
        Promotion {
            id: Uuid::parse_str("60000000-0000-0000-0000-000000000001").unwrap(),
            code: Some("20OFF".to_string()),
            name: "20% off basket".to_string(),
            promo_type: PromotionType::PercentageOff { percent_bps: 2000 },
            priority: 10,
            valid_from: Utc::now(),
            valid_until: None,
            conditions: vec![PromoCondition::MinBasketAmount { amount_cents: 1 }],
            actions: vec![PromoAction::ApplyToBasket],
            version: 1,
        },
        Promotion {
            id: Uuid::parse_str("60000000-0000-0000-0000-000000000002").unwrap(),
            code: Some("BUY2_50".to_string()),
            name: "Buy 2 of Example Product One get 50% off each".to_string(),
            promo_type: PromotionType::PercentageOff { percent_bps: 5000 },
            priority: 20,
            valid_from: Utc::now(),
            valid_until: None,
            conditions: vec![PromoCondition::ItemInBasket {
                item_id: item_demo_001,
                min_quantity: 2,
            }],
            actions: vec![PromoAction::ApplyToItem {
                item_id: item_demo_001,
                max_quantity: Some(2),
            }],
            version: 1,
        },
    ]
}

fn demo_customers() -> Vec<Customer> {
    vec![Customer {
        id: Uuid::parse_str("70000000-0000-0000-0000-000000000001").unwrap(),
        code: "CUST01".to_string(),
        name: "Demo Customer".to_string(),
        email: Some("demo@example.com".to_string()),
        version: 1,
    }]
}

fn demo_inventory_levels_for_catalog(items: &[CatalogItem]) -> Vec<InventoryLevel> {
    items
        .iter()
        .enumerate()
        .map(|(idx, item)| InventoryLevel {
            item_id: item.id,
            available_qty: ((idx % 20) as i64 + 1) * 5,
            is_available: true,
            image_urls: demo_inventory_image_urls(idx),
            version: 1,
        })
        .collect()
}

fn demo_inventory_image_urls(idx: usize) -> Vec<String> {
    const COLORS: [&str; 12] = [
        "ef4444", "f97316", "f59e0b", "84cc16", "22c55e", "10b981", "14b8a6", "06b6d4", "0ea5e9",
        "3b82f6", "6366f1", "ec4899",
    ];
    let product_num = idx + 1;
    let a = COLORS[idx % COLORS.len()];
    let b = COLORS[(idx * 7 + 3) % COLORS.len()];
    let second = if b == a {
        COLORS[(idx + 5) % COLORS.len()]
    } else {
        b
    };
    vec![
        format!("https://dummyimage.com/600x600/{a}/ffffff.png&text=Product+{product_num}+A"),
        format!("https://dummyimage.com/600x600/{second}/ffffff.png&text=Product+{product_num}+B"),
    ]
}

fn demo_coupons() -> Vec<CouponDefinition> {
    vec![CouponDefinition {
        id: Uuid::parse_str("80000000-0000-0000-0000-000000000001").unwrap(),
        code: "SAVE20".to_string(),
        promo_id: Uuid::parse_str("60000000-0000-0000-0000-000000000001").unwrap(),
        max_redemptions_total: Some(100),
        max_redemptions_per_customer: Some(1),
        valid_from: Utc::now(),
        valid_until: None,
        version: 1,
    }]
}

fn demo_print_templates() -> Vec<apex_edge_contracts::PrintTemplateConfig> {
    use apex_edge_contracts::{DocumentType, PrintTemplateConfig};
    vec![
        PrintTemplateConfig {
            id: Uuid::parse_str("90000000-0000-0000-0000-000000000001").unwrap(),
            document_type: DocumentType::CustomerReceipt,
            template_body: r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Receipt</title></head><body><h1>Receipt</h1><p>Order: {{order_id}}</p><p>Total: {{total_cents}} cents</p></body></html>"#.to_string(),
            version: 1,
        },
        PrintTemplateConfig {
            id: Uuid::parse_str("90000000-0000-0000-0000-000000000002").unwrap(),
            document_type: DocumentType::GiftReceipt,
            template_body: r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Gift Receipt</title></head><body><h1>Gift Receipt</h1><p>Order: {{order_id}}</p><p>Total: {{total_cents}} cents</p></body></html>"#.to_string(),
            version: 1,
        },
    ]
}
