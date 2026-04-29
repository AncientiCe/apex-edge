//! Read-only order ledger API.

use apex_edge_storage::{fetch_order_ledger_entry, list_order_ledger_entries};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;

#[derive(Debug, Clone, Deserialize)]
pub struct OrderListQuery {
    pub shift_id: Option<Uuid>,
}

pub async fn get_order_handler(
    State(app): State<AppState>,
    Path(order_id): Path<Uuid>,
) -> Result<Json<apex_edge_storage::OrderLedgerEntry>, StatusCode> {
    match fetch_order_ledger_entry(&app.pool, order_id).await {
        Ok(Some(order)) => {
            metrics::counter!(
                apex_edge_metrics::ORDERS_LOOKUP_TOTAL,
                1u64,
                "operation" => "get_order",
                "outcome" => apex_edge_metrics::OUTCOME_HIT
            );
            Ok(Json(order))
        }
        Ok(None) => {
            metrics::counter!(
                apex_edge_metrics::ORDERS_LOOKUP_TOTAL,
                1u64,
                "operation" => "get_order",
                "outcome" => apex_edge_metrics::OUTCOME_NOT_FOUND
            );
            Err(StatusCode::NOT_FOUND)
        }
        Err(_) => {
            metrics::counter!(
                apex_edge_metrics::ORDERS_LOOKUP_TOTAL,
                1u64,
                "operation" => "get_order",
                "outcome" => apex_edge_metrics::OUTCOME_ERROR
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub async fn list_orders_handler(
    State(app): State<AppState>,
    Query(query): Query<OrderListQuery>,
) -> Result<Json<Vec<apex_edge_storage::OrderLedgerSummary>>, StatusCode> {
    match list_order_ledger_entries(&app.pool, app.store_id, query.shift_id).await {
        Ok(orders) => {
            metrics::counter!(
                apex_edge_metrics::ORDERS_LOOKUP_TOTAL,
                1u64,
                "operation" => "list_orders",
                "outcome" => apex_edge_metrics::OUTCOME_SUCCESS
            );
            Ok(Json(orders))
        }
        Err(_) => {
            metrics::counter!(
                apex_edge_metrics::ORDERS_LOOKUP_TOTAL,
                1u64,
                "operation" => "list_orders",
                "outcome" => apex_edge_metrics::OUTCOME_ERROR
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
