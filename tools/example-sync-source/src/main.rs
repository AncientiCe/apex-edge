//! Example sync source: serves NDJSON streams per entity for ApexEdge to pull.
//! Contract: first line = {"total": N}, then N lines of JSON (base64-encoded payload string per line).
//! Run with: cargo run -p example-sync-source
//! Listen port: 3030 by default (env SYNC_SOURCE_PORT).

use apex_edge_contracts::{
    CatalogItem, Category, CouponDefinition, Customer, InventoryLevel, PriceBook, PriceBookEntry,
    PromoAction, PromoCondition, Promotion, PromotionType, TaxRule,
};
use axum::{body::Body, extract::State, http::Response, routing::get, Router};
use base64::Engine;
use chrono::Utc;
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

fn default_store_id() -> Uuid {
    Uuid::nil()
}

#[cfg(test)]
mod tests {
    use super::{demo_catalog_items, demo_price_book_for_catalog};

    #[test]
    fn price_book_covers_every_catalog_item() {
        let items = demo_catalog_items();
        let book = demo_price_book_for_catalog(&items);

        assert_eq!(book.entries.len(), items.len());
        for item in &items {
            assert!(
                book.entries
                    .iter()
                    .any(|entry| entry.item_id == item.id && entry.modifier_option_id.is_none()),
                "missing price entry for item {}",
                item.id
            );
        }
    }
}

/// Shared state for handlers (store_id for scoping example data).
struct AppState {
    store_id: Uuid,
}

#[derive(Serialize)]
struct NdjsonMeta {
    total: u64,
}

/// Emit NDJSON body: first line = {"total": N}, then N lines each a JSON string (base64 payload).
fn ndjson_body(lines: Vec<String>) -> Body {
    let total = lines.len() as u64;
    let mut out = serde_json::to_string(&NdjsonMeta { total }).unwrap();
    out.push('\n');
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    Body::from(out)
}

fn b64(s: &[u8]) -> String {
    serde_json::to_string(&base64::engine::general_purpose::STANDARD.encode(s)).unwrap()
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

fn demo_price_book_for_catalog(items: &[CatalogItem]) -> PriceBook {
    PriceBook {
        id: Uuid::parse_str("40000000-0000-0000-0000-000000000001").unwrap(),
        name: "Default".into(),
        effective_from: Utc::now(),
        effective_until: None,
        entries: items
            .iter()
            .enumerate()
            .map(|(idx, item)| PriceBookEntry {
                item_id: item.id,
                modifier_option_id: None,
                price_cents: 199 + ((idx as u64 % 17) * 37),
                currency: "USD".into(),
            })
            .collect(),
        version: 1,
    }
}

async fn ndjson_catalog(State(state): State<Arc<AppState>>) -> Response<Body> {
    eprintln!("catalog request store_id={}", state.store_id);
    let items = demo_catalog_items();

    let lines: Vec<String> = items
        .iter()
        .map(|i| b64(&serde_json::to_vec(i).unwrap()))
        .collect();

    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(ndjson_body(lines))
        .unwrap()
}

async fn ndjson_categories(State(state): State<Arc<AppState>>) -> Response<Body> {
    let _ = state;
    let categories: Vec<Category> = vec![Category {
        id: Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap(),
        name: "Demo Category".into(),
        parent_id: None,
        sort_order: 0,
        version: 1,
    }];
    let lines: Vec<String> = categories
        .iter()
        .map(|c| b64(&serde_json::to_vec(c).unwrap()))
        .collect();
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(ndjson_body(lines))
        .unwrap()
}

async fn ndjson_price_book(State(state): State<Arc<AppState>>) -> Response<Body> {
    let _ = state;
    let items = demo_catalog_items();
    let book = demo_price_book_for_catalog(&items);
    let lines = vec![b64(&serde_json::to_vec(&book).unwrap())];
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(ndjson_body(lines))
        .unwrap()
}

async fn ndjson_tax_rules(State(state): State<Arc<AppState>>) -> Response<Body> {
    let _ = state;
    let tax_id = Uuid::parse_str("20000000-0000-0000-0000-000000000002").unwrap();
    let rules: Vec<TaxRule> = vec![TaxRule {
        id: Uuid::parse_str("50000000-0000-0000-0000-000000000001").unwrap(),
        tax_category_id: tax_id,
        rate_bps: 0,
        name: "No tax".into(),
        inclusive: false,
        version: 1,
    }];
    let lines: Vec<String> = rules
        .iter()
        .map(|r| b64(&serde_json::to_vec(r).unwrap()))
        .collect();
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(ndjson_body(lines))
        .unwrap()
}

async fn ndjson_promotions(State(state): State<Arc<AppState>>) -> Response<Body> {
    eprintln!("promotions request store_id={}", state.store_id);
    let item_demo_001 = Uuid::parse_str("30000000-0000-0000-0000-000000000001").unwrap();
    let promos = [
        Promotion {
            id: Uuid::parse_str("60000000-0000-0000-0000-000000000001").unwrap(),
            code: Some("20OFF".into()),
            name: "20% off basket".into(),
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
            code: Some("BUY2_50".into()),
            name: "Buy 2 of Example Product One get 50% off each".into(),
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
    ];
    let lines: Vec<String> = promos
        .iter()
        .map(|p| b64(&serde_json::to_vec(p).unwrap()))
        .collect();
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(ndjson_body(lines))
        .unwrap()
}

async fn ndjson_customers(State(state): State<Arc<AppState>>) -> Response<Body> {
    let _ = state;
    let customers: Vec<Customer> = vec![Customer {
        id: Uuid::parse_str("70000000-0000-0000-0000-000000000001").unwrap(),
        code: "CUST01".into(),
        name: "Demo Customer".into(),
        email: Some("demo@example.com".into()),
        version: 1,
    }];
    let lines: Vec<String> = customers
        .iter()
        .map(|c| b64(&serde_json::to_vec(c).unwrap()))
        .collect();
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(ndjson_body(lines))
        .unwrap()
}

async fn ndjson_inventory(State(state): State<Arc<AppState>>) -> Response<Body> {
    let _ = state;
    let items = demo_catalog_items();
    let levels: Vec<InventoryLevel> = items
        .iter()
        .enumerate()
        .map(|(idx, item)| InventoryLevel {
            item_id: item.id,
            available_qty: ((idx % 20) as i64 + 1) * 5,
            is_available: true,
            image_urls: vec![
                format!(
                    "https://via.placeholder.com/400x400?text=Product+{}",
                    idx + 1
                ),
                format!(
                    "https://via.placeholder.com/400x400/0055ff/ffffff?text=Product+{idx}+View+2"
                ),
            ],
            version: 1,
        })
        .collect();
    let lines: Vec<String> = levels
        .iter()
        .map(|l| b64(&serde_json::to_vec(l).unwrap()))
        .collect();
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(ndjson_body(lines))
        .unwrap()
}

async fn ndjson_coupons(State(state): State<Arc<AppState>>) -> Response<Body> {
    let _ = state;
    let promo_id = Uuid::parse_str("60000000-0000-0000-0000-000000000001").unwrap();
    let def = CouponDefinition {
        id: Uuid::parse_str("80000000-0000-0000-0000-000000000001").unwrap(),
        code: "SAVE20".into(),
        promo_id,
        max_redemptions_total: Some(100),
        max_redemptions_per_customer: Some(1),
        valid_from: Utc::now(),
        valid_until: None,
        version: 1,
    };
    let lines = vec![b64(&serde_json::to_vec(&def).unwrap())];
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(ndjson_body(lines))
        .unwrap()
}

async fn ndjson_print_templates(State(_state): State<Arc<AppState>>) -> Response<Body> {
    use apex_edge_contracts::{DocumentType, PrintTemplateConfig};
    let customer_receipt = PrintTemplateConfig {
        id: Uuid::parse_str("90000000-0000-0000-0000-000000000001").unwrap(),
        document_type: DocumentType::CustomerReceipt,
        template_body: r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Receipt</title></head><body><h1>Receipt</h1><p>Order: {{order_id}}</p><p>Total: {{total_cents}} cents</p><table><thead><tr><th>Item</th><th>Qty</th><th>Total</th></tr></thead><tbody>{{#each lines}}<tr><td>{{name}}</td><td>{{quantity}}</td><td>{{line_total_cents}}</td></tr>{{/each}}</tbody></table><p>Subtotal: {{subtotal_cents}} | Tax: {{tax_cents}} | Discount: {{discount_cents}}</p></body></html>"#.into(),
        version: 1,
    };
    let gift_receipt = PrintTemplateConfig {
        id: Uuid::parse_str("90000000-0000-0000-0000-000000000002").unwrap(),
        document_type: DocumentType::GiftReceipt,
        template_body: r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Gift Receipt</title></head><body><h1>Gift Receipt</h1><p>Order: {{order_id}}</p><p>Total: {{total_cents}} cents</p></body></html>"#.into(),
        version: 1,
    };
    let lines = vec![
        b64(&serde_json::to_vec(&customer_receipt).unwrap()),
        b64(&serde_json::to_vec(&gift_receipt).unwrap()),
    ];
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(ndjson_body(lines))
        .unwrap()
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("SYNC_SOURCE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3030);

    let state = Arc::new(AppState {
        store_id: default_store_id(),
    });

    let app = Router::new()
        .route("/sync/ndjson/catalog", get(ndjson_catalog))
        .route("/sync/ndjson/categories", get(ndjson_categories))
        .route("/sync/ndjson/price_book", get(ndjson_price_book))
        .route("/sync/ndjson/tax_rules", get(ndjson_tax_rules))
        .route("/sync/ndjson/promotions", get(ndjson_promotions))
        .route("/sync/ndjson/customers", get(ndjson_customers))
        .route("/sync/ndjson/coupons", get(ndjson_coupons))
        .route("/sync/ndjson/inventory", get(ndjson_inventory))
        .route("/sync/ndjson/print_templates", get(ndjson_print_templates))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    println!("Example sync source listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
