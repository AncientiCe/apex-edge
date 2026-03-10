//! Catalog search endpoint for product lookup by SKU.

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
    pub sku: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProductSearchResult {
    pub id: Uuid,
    pub sku: String,
    pub name: String,
    pub category_id: Uuid,
    pub tax_category_id: Uuid,
}

pub async fn search_products(
    State(state): State<AppState>,
    Query(q): Query<ProductSearchQuery>,
) -> Result<Json<Vec<ProductSearchResult>>, StatusCode> {
    let Some(sku) = q.sku.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(Json(vec![]));
    };
    let row = apex_edge_storage::get_catalog_item_by_sku(&state.pool, state.store_id, sku)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        row.map(|r| ProductSearchResult {
            id: r.id,
            sku: r.sku,
            name: r.name,
            category_id: r.category_id,
            tax_category_id: r.tax_category_id,
        })
        .into_iter()
        .collect(),
    ))
}
