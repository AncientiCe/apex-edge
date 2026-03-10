//! Catalog categories for POS browsing.

use axum::{extract::State, Json};
use uuid::Uuid;

use crate::pos::AppState;

#[derive(Debug, serde::Serialize)]
pub struct CategoryResult {
    pub id: Uuid,
    pub name: String,
}

pub async fn list_categories(
    State(state): State<AppState>,
) -> Result<Json<Vec<CategoryResult>>, axum::http::StatusCode> {
    let rows = apex_edge_storage::list_categories(&state.pool, state.store_id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        rows.into_iter()
            .map(|r| CategoryResult {
                id: r.id,
                name: r.name,
            })
            .collect(),
    ))
}
