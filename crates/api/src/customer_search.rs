//! Customer search endpoint for POS: by code, name, email, or id.

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
    /// Search by name, email, code, or id (substring match for name/email, exact for code/id).
    pub q: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct CustomerSearchResult {
    pub id: Uuid,
    pub code: String,
    pub name: String,
    pub email: Option<String>,
}

pub async fn search_customers(
    State(state): State<AppState>,
    Query(query): Query<CustomerSearchQuery>,
) -> Result<Json<Vec<CustomerSearchResult>>, StatusCode> {
    if let Some(code) = query.code.as_deref().filter(|s| !s.is_empty()) {
        let row = apex_edge_storage::get_customer_by_code(&state.pool, state.store_id, code)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(Json(
            row.map(|r| CustomerSearchResult {
                id: r.id,
                code: r.code.clone(),
                name: r.name.clone(),
                email: r.email.clone(),
            })
            .into_iter()
            .collect(),
        ));
    }
    if let Some(q) = query.q.as_deref().filter(|s| !s.trim().is_empty()) {
        let rows = apex_edge_storage::search_customers(&state.pool, state.store_id, q)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(Json(
            rows.into_iter()
                .map(|r| CustomerSearchResult {
                    id: r.id,
                    code: r.code,
                    name: r.name,
                    email: r.email,
                })
                .collect(),
        ));
    }
    Ok(Json(vec![]))
}
