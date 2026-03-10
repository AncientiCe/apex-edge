//! Catalog endpoints for POS: product search/list by SKU, name, description; categories; pagination.

use axum::{
    extract::{Query, State},
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
}

#[derive(Debug, serde::Serialize)]
pub struct ProductListResponse {
    pub items: Vec<ProductSearchResult>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

pub async fn search_products(
    State(state): State<AppState>,
    Query(query): Query<ProductSearchQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if let Some(sku) = query.sku.as_deref().filter(|s| !s.is_empty()) {
        let row = apex_edge_storage::get_catalog_item_by_sku(&state.pool, state.store_id, sku)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let arr: Vec<ProductSearchResult> = row
            .map(|r| ProductSearchResult {
                id: r.id,
                sku: r.sku,
                name: r.name,
                category_id: r.category_id,
                tax_category_id: r.tax_category_id,
                description: r.description,
            })
            .into_iter()
            .collect();
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
    let items = items
        .into_iter()
        .map(|r| ProductSearchResult {
            id: r.id,
            sku: r.sku,
            name: r.name,
            category_id: r.category_id,
            tax_category_id: r.tax_category_id,
            description: r.description,
        })
        .collect();
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
