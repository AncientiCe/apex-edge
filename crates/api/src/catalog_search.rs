//! Catalog endpoints for POS: product search/list by SKU, name, description; categories; pagination.
//! Also exposes availability (is_active, available_qty, image_urls) from synced inventory levels.

use apex_edge_metrics::{
    CATALOG_PRODUCT_BY_ID_TOTAL, OUTCOME_ERROR, OUTCOME_HIT, OUTCOME_NOT_FOUND,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::pos::AppState;

#[derive(Debug, Deserialize)]
pub struct ProductSearchQuery {
    /// Exact SKU lookup (returns at most one; backward compatible).
    pub sku: Option<String>,
    /// Search/list: match SKU, name, or description; use with category_id, page, per_page.
    pub q: Option<String>,
    pub category_id: Option<String>,
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_page() -> u32 {
    1
}
fn default_per_page() -> u32 {
    24
}

#[derive(Debug, serde::Serialize)]
pub struct ProductSearchResult {
    pub id: Uuid,
    pub sku: String,
    pub name: String,
    pub category_id: Uuid,
    pub tax_category_id: Uuid,
    pub description: Option<String>,
    pub is_active: bool,
    /// `None` when inventory has not been synced (stock untracked).
    pub available_qty: Option<i64>,
    pub image_urls: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProductListResponse {
    pub items: Vec<ProductSearchResult>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

fn to_product_result(r: apex_edge_storage::CatalogItemRow) -> ProductSearchResult {
    ProductSearchResult {
        id: r.id,
        sku: r.sku,
        name: r.name,
        category_id: r.category_id,
        tax_category_id: r.tax_category_id,
        description: r.description,
        is_active: r.is_active,
        available_qty: r.available_qty,
        image_urls: r.image_urls,
    }
}

pub async fn search_products(
    State(state): State<AppState>,
    Query(query): Query<ProductSearchQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if let Some(sku) = query.sku.as_deref().filter(|s| !s.is_empty()) {
        let row = apex_edge_storage::get_catalog_item_by_sku(&state.pool, state.store_id, sku)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let arr: Vec<ProductSearchResult> = row.map(to_product_result).into_iter().collect();
        return Ok(Json(
            serde_json::to_value(arr).unwrap_or(serde_json::json!([])),
        ));
    }
    let category_id = query
        .category_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());
    let per_page = query.per_page.clamp(1, 100);
    let page = query.page.max(1);
    let offset = (page - 1) * per_page;
    let q = query.q.as_deref();
    let (items, total) = apex_edge_storage::list_catalog_items(
        &state.pool,
        state.store_id,
        category_id,
        q,
        per_page,
        offset,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let items = items.into_iter().map(to_product_result).collect();
    Ok(Json(
        serde_json::to_value(ProductListResponse {
            items,
            total,
            page,
            per_page,
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    ))
}

/// Fetch a single product by its UUID. Returns full product detail including
/// availability and all image URLs for the PDP gallery.
pub async fn get_product_by_id(
    State(state): State<AppState>,
    Path(product_id): Path<Uuid>,
) -> Result<Json<ProductSearchResult>, StatusCode> {
    let result = apex_edge_storage::get_catalog_item(&state.pool, state.store_id, product_id).await;
    match result {
        Ok(Some(row)) => {
            metrics::counter!(CATALOG_PRODUCT_BY_ID_TOTAL, 1u64, "outcome" => OUTCOME_HIT);
            Ok(Json(to_product_result(row)))
        }
        Ok(None) => {
            metrics::counter!(CATALOG_PRODUCT_BY_ID_TOTAL, 1u64, "outcome" => OUTCOME_NOT_FOUND);
            Err(StatusCode::NOT_FOUND)
        }
        Err(_) => {
            metrics::counter!(CATALOG_PRODUCT_BY_ID_TOTAL, 1u64, "outcome" => OUTCOME_ERROR);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
