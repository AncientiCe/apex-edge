//! Example sync source: serves NDJSON streams per entity for ApexEdge to pull.
//! Contract: first line = {"total": N}, then N lines of JSON (base64-encoded payload string per line).
//! Run with: cargo run -p example-sync-source
//! Listen port: 3030 by default (env SYNC_SOURCE_PORT).

use apex_edge_contracts::{
    CatalogItem, Category, CouponDefinition, PriceBook, PriceBookEntry, PromoAction,
    PromoCondition, Promotion, PromotionType, TaxRule,
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

async fn ndjson_catalog(State(state): State<Arc<AppState>>) -> Response<Body> {
    eprintln!("catalog request store_id={}", state.store_id);
    let cat_id = Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
    let tax_id = Uuid::parse_str("20000000-0000-0000-0000-000000000002").unwrap();

    let items: Vec<CatalogItem> = vec![
        CatalogItem {
            id: Uuid::parse_str("30000000-0000-0000-0000-000000000001").unwrap(),
            sku: "DEMO-001".into(),
            name: "Example Product One".into(),
            description: Some("Description one".into()),
            category_id: cat_id,
            tax_category_id: tax_id,
            modifiers: vec![],
            is_active: true,
            version: 1,
        },
        CatalogItem {
            id: Uuid::parse_str("30000000-0000-0000-0000-000000000002").unwrap(),
            sku: "DEMO-002".into(),
            name: "Example Product Two".into(),
            description: None,
            category_id: cat_id,
            tax_category_id: tax_id,
            modifiers: vec![],
            is_active: true,
            version: 1,
        },
    ];

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
    let item1 = Uuid::parse_str("30000000-0000-0000-0000-000000000001").unwrap();
    let item2 = Uuid::parse_str("30000000-0000-0000-0000-000000000002").unwrap();
    let book = PriceBook {
        id: Uuid::parse_str("40000000-0000-0000-0000-000000000001").unwrap(),
        name: "Default".into(),
        effective_from: Utc::now(),
        effective_until: None,
        entries: vec![
            PriceBookEntry {
                item_id: item1,
                modifier_option_id: None,
                price_cents: 1000,
                currency: "USD".into(),
            },
            PriceBookEntry {
                item_id: item2,
                modifier_option_id: None,
                price_cents: 500,
                currency: "USD".into(),
            },
        ],
        version: 1,
    };
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

#[derive(Serialize)]
struct ExampleCustomer {
    id: Uuid,
    store_id: Uuid,
    code: String,
    name: String,
    email: Option<String>,
}

async fn ndjson_customers(State(state): State<Arc<AppState>>) -> Response<Body> {
    let store_id = state.store_id;
    let customers: Vec<ExampleCustomer> = vec![ExampleCustomer {
        id: Uuid::parse_str("70000000-0000-0000-0000-000000000001").unwrap(),
        store_id,
        code: "CUST01".into(),
        name: "Demo Customer".into(),
        email: Some("demo@example.com".into()),
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
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    println!("Example sync source listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
