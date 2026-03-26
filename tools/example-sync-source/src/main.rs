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
use rusqlite::Connection;
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

fn default_store_id() -> Uuid {
    Uuid::nil()
}

#[cfg(test)]
mod tests {
    use super::{
        demo_catalog_items, demo_inventory_levels_for_catalog, demo_price_book_for_catalog,
        parse_variant_identifiers,
    };

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

    #[test]
    fn inventory_includes_two_distinct_placeholder_images_per_product() {
        let items = demo_catalog_items();
        let levels = demo_inventory_levels_for_catalog(&items);

        assert_eq!(levels.len(), items.len());
        for level in levels {
            assert_eq!(
                level.image_urls.len(),
                2,
                "expected two image URLs per product"
            );
            assert_ne!(
                level.image_urls[0], level.image_urls[1],
                "placeholder image URLs should be distinct"
            );
            assert!(
                level
                    .image_urls
                    .iter()
                    .all(|url| url.starts_with("https://dummyimage.com/")),
                "placeholder URLs should use dummyimage.com"
            );
        }
    }

    #[test]
    fn parse_variant_identifiers_extracts_expected_fields() {
        let ids = parse_variant_identifiers("ean13=8103800957016,sku=10055359980,gtin=123");
        assert_eq!(ids.sku.as_deref(), Some("10055359980"));
        assert_eq!(ids.ean13.as_deref(), Some("8103800957016"));
        assert_eq!(ids.gtin.as_deref(), Some("123"));
    }
}

/// Shared state for handlers (store_id for scoping example data).
struct AppState {
    store_id: Uuid,
    items: Vec<CatalogItem>,
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

#[derive(Debug)]
struct SqliteProductRow {
    product_id: String,
    product_name: String,
    variant_identifiers: Option<String>,
}

fn parse_variant_identifiers(value: &str) -> apex_edge_contracts::ExternalIdentifiers {
    let mut out = apex_edge_contracts::ExternalIdentifiers::default();
    for pair in value.split(',') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default().trim();
        let val = parts.next().unwrap_or_default().trim();
        if key.is_empty() || val.is_empty() {
            continue;
        }
        match key {
            "sku" => out.sku = Some(val.to_string()),
            "gtin" => out.gtin = Some(val.to_string()),
            "upc" => out.upc = Some(val.to_string()),
            "ean13" => out.ean13 = Some(val.to_string()),
            "jan" => out.jan = Some(val.to_string()),
            "isbn" => out.isbn = Some(val.to_string()),
            _ => {}
        }
    }
    out
}

fn catalog_item_from_sqlite_row(
    row: SqliteProductRow,
    category_id: Uuid,
    tax_category_id: Uuid,
) -> CatalogItem {
    let external_identifiers = row
        .variant_identifiers
        .as_deref()
        .map(parse_variant_identifiers);
    let sku = external_identifiers
        .as_ref()
        .and_then(|ids| ids.sku.clone())
        .unwrap_or_else(|| row.product_id.clone());
    CatalogItem {
        id: Uuid::new_v5(&Uuid::NAMESPACE_OID, row.product_id.as_bytes()),
        sku,
        name: row.product_name.clone(),
        description: Some(format!("Imported from SQLite catalog ({})", row.product_id)),
        category_id,
        tax_category_id,
        modifiers: vec![],
        is_active: true,
        title: Some(row.product_name),
        brand: None,
        caption: None,
        external_identifiers,
        images: None,
        is_preorder: None,
        online_from: None,
        serialized_inventory: None,
        extended_attributes: None,
        variations: None,
        variation_attributes: None,
        version: 1,
    }
}

fn load_catalog_items_from_sqlite(path: &str) -> Result<Vec<CatalogItem>, String> {
    let conn = Connection::open(path).map_err(|e| format!("open sqlite at {path}: {e}"))?;
    let mut stmt = conn
        .prepare("SELECT productId, productName, variantIdentifiers FROM products")
        .map_err(|e| format!("prepare products query: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SqliteProductRow {
                product_id: row.get(0)?,
                product_name: row.get(1)?,
                variant_identifiers: row.get(2).ok(),
            })
        })
        .map_err(|e| format!("query products: {e}"))?;

    let category_id = Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
    let tax_category_id = Uuid::parse_str("20000000-0000-0000-0000-000000000002").unwrap();
    let mut items = Vec::new();
    for row in rows {
        let row = row.map_err(|e| format!("read product row: {e}"))?;
        items.push(catalog_item_from_sqlite_row(
            row,
            category_id,
            tax_category_id,
        ));
    }
    Ok(items)
}

fn load_catalog_items() -> Vec<CatalogItem> {
    let db_path = std::env::var("EXAMPLE_SYNC_CATALOG_DB")
        .ok()
        .or_else(|| std::env::var("ASSOCIATE_APP_CATALOG_DB").ok());
    if let Some(path) = db_path {
        match load_catalog_items_from_sqlite(path.as_str()) {
            Ok(items) if !items.is_empty() => {
                eprintln!("Loaded {} products from SQLite catalog export", items.len());
                return items;
            }
            Ok(_) => {
                eprintln!("SQLite catalog export had no products, falling back to demo catalog");
            }
            Err(err) => {
                eprintln!(
                    "Failed to load SQLite catalog export ({err}), falling back to demo catalog"
                );
            }
        }
    }
    demo_catalog_items()
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

async fn ndjson_catalog(State(state): State<Arc<AppState>>) -> Response<Body> {
    eprintln!("catalog request store_id={}", state.store_id);
    let items = &state.items;

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
    let items = &state.items;
    let book = demo_price_book_for_catalog(items);
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
    let items = &state.items;
    let levels = demo_inventory_levels_for_catalog(items);
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
        items: load_catalog_items(),
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
