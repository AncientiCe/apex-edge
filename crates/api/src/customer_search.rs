//! Customer search endpoint for lookup by code.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::pos::AppState;

#[derive(Debug, Deserialize)]
pub struct CustomerSearchQuery {
    pub code: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct CustomerSearchResult {
    pub id: Uuid,
    pub code: String,
    pub name: String,
}

pub async fn search_customers(
    State(state): State<AppState>,
    Query(q): Query<CustomerSearchQuery>,
) -> Result<Json<Vec<CustomerSearchResult>>, StatusCode> {
    let Some(code) = q.code.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(Json(vec![]));
    };
    let row = apex_edge_storage::get_customer_by_code(&state.pool, state.store_id, code)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        row.map(|r| CustomerSearchResult {
            id: r.id,
            code: r.code,
            name: r.name,
        })
        .into_iter()
        .collect(),
    ))
}
