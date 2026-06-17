//! Real-time inventory helpers shared by POS command handlers.
//!
//! These wrap the storage ledger with metric emission and `StockChanged` broadcasts so
//! every register in a store sees live availability the moment stock moves.

use apex_edge_metrics::{INVENTORY_OVERSELL_PREVENTED_TOTAL, INVENTORY_RESERVATIONS_TOTAL};
use apex_edge_storage::available_to_sell_for_items;
use uuid::Uuid;

use crate::stream::{stream_broadcast, StreamKind};
use crate::AppState;

/// Record the outcome of a reservation attempt as a metric.
pub fn record_reservation_outcome(outcome: &'static str) {
    metrics::counter!(INVENTORY_RESERVATIONS_TOTAL, 1u64, "outcome" => outcome);
}

/// Record that the ledger prevented an oversell.
pub fn record_oversell_prevented() {
    metrics::counter!(INVENTORY_OVERSELL_PREVENTED_TOTAL, 1u64);
}

/// Broadcast live `available_to_sell` for the given items to all connected registers.
/// Untracked items are silently skipped (no ledger row, no constraint).
pub async fn broadcast_stock_changed(app: &AppState, store_id: Uuid, item_ids: &[Uuid]) {
    if item_ids.is_empty() {
        return;
    }
    let map = match available_to_sell_for_items(&app.pool, store_id, item_ids).await {
        Ok(m) => m,
        Err(_) => return,
    };
    if map.is_empty() {
        return;
    }
    let items: Vec<serde_json::Value> = map
        .into_iter()
        .map(|(item_id, available)| {
            serde_json::json!({
                "item_id": item_id.to_string(),
                "available_to_sell": available,
            })
        })
        .collect();
    stream_broadcast(
        app,
        store_id,
        StreamKind::StockChanged,
        serde_json::json!({ "items": items }),
    )
    .await;
}
