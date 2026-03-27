use std::sync::Arc;
use std::time::Instant;

use apex_edge_contracts::{HqOrderSubmissionEnvelope, HqOrderSubmissionResponse};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    routing::post,
    Json, Router,
};
use metrics::{counter, histogram};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/orders", post(post_order).get(list_orders))
        .route("/api/orders/:submission_id", get(get_order))
}

#[derive(Debug, Deserialize)]
struct PaginationParams {
    page: Option<u64>,
    per_page: Option<u64>,
}

#[derive(Debug, Serialize)]
struct OrderDetailApiResponse {
    #[serde(flatten)]
    detail: crate::storage::StoredOrderDetail,
    payload: serde_json::Value,
}

async fn post_order(
    State(state): State<Arc<AppState>>,
    Json(envelope): Json<HqOrderSubmissionEnvelope>,
) -> impl IntoResponse {
    let started = Instant::now();
    let result = state.storage.insert_order(&envelope);
    histogram!(
        "fake_hq_order_receive_duration_seconds",
        started.elapsed().as_secs_f64()
    );

    match result {
        Ok(insert_result) => {
            counter!("fake_hq_orders_received_total", 1);
            if !insert_result.inserted {
                counter!("fake_hq_orders_duplicate_total", 1);
            }
            (
                StatusCode::OK,
                Json(HqOrderSubmissionResponse {
                    accepted: true,
                    submission_id: envelope.submission_id,
                    order_id: envelope.order.order_id,
                    hq_order_ref: Some(format!("FAKE-HQ-{}", envelope.order.order_id)),
                    errors: Vec::new(),
                }),
            )
                .into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "failed_to_store_order",
                "message": err
            })),
        )
            .into_response(),
    }
}

async fn list_orders(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(1);
    let per_page = params.per_page.unwrap_or(20);
    match state.storage.list_orders(page, per_page) {
        Ok(page_result) => (StatusCode::OK, Json(serde_json::json!(page_result))).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "failed_to_list_orders",
                "message": err
            })),
        )
            .into_response(),
    }
}

async fn get_order(
    Path(submission_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let submission_id = match Uuid::parse_str(&submission_id) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error":"invalid_submission_id"
                })),
            )
                .into_response();
        }
    };

    match state.storage.get_order(submission_id) {
        Ok(Some(detail)) => {
            let payload =
                serde_json::from_str::<serde_json::Value>(&detail.payload_json).unwrap_or_default();
            (
                StatusCode::OK,
                Json(serde_json::json!(OrderDetailApiResponse {
                    detail,
                    payload
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error":"order_not_found"
            })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error":"failed_to_get_order",
                "message": err
            })),
        )
            .into_response(),
    }
}
