//! POS command handlers: validate envelope, apply command, return cart state or finalize result.

use apex_edge_contracts::{
    CartState, ContractVersion, PosCommand, PosError, PosRequestEnvelope, PosResponseEnvelope,
};
use apex_edge_metrics::{
    OUTCOME_SUCCESS, OUTCOME_UNSUPPORTED_VERSION, POS_COMMANDS_TOTAL, POS_COMMAND_DURATION_SECONDS,
};
use axum::{
    extract::{Path, State},
    Json,
};
use std::time::Instant;
use uuid::Uuid;

fn pos_operation_label(cmd: &PosCommand) -> &'static str {
    match cmd {
        PosCommand::CreateCart(_) => "create_cart",
        PosCommand::SetCustomer(_) => "set_customer",
        PosCommand::AddLineItem(_) => "add_line_item",
        PosCommand::UpdateLineItem(_) => "update_line_item",
        PosCommand::RemoveLineItem(_) => "remove_line_item",
        PosCommand::ApplyPromo(_) => "apply_promo",
        PosCommand::RemovePromo(_) => "remove_promo",
        PosCommand::ApplyCoupon(_) => "apply_coupon",
        PosCommand::RemoveCoupon(_) => "remove_coupon",
        PosCommand::ApplyManualDiscount(_) => "apply_manual_discount",
        PosCommand::SetTendering(_) => "set_tendering",
        PosCommand::AddPayment(_) => "add_payment",
        PosCommand::FinalizeOrder(_) => "finalize_order",
        PosCommand::VoidCart(_) => "void_cart",
        PosCommand::StartReturn(_) => "start_return",
        PosCommand::ReturnLineItem(_) => "return_line_item",
        PosCommand::RefundTender(_) => "refund_tender",
        PosCommand::FinalizeReturn(_) => "finalize_return",
        PosCommand::VoidReturn(_) => "void_return",
        PosCommand::OpenTill(_) => "open_till",
        PosCommand::PaidIn(_) => "paid_in",
        PosCommand::PaidOut(_) => "paid_out",
        PosCommand::NoSale(_) => "no_sale",
        PosCommand::CashCount(_) => "cash_count",
        PosCommand::GetXReport(_) => "get_x_report",
        PosCommand::CloseTill(_) => "close_till",
        PosCommand::ParkCart(_) => "park_cart",
        PosCommand::RecallCart(_) => "recall_cart",
        PosCommand::ListParkedCarts(_) => "list_parked_carts",
        PosCommand::ClockIn(_) => "clock_in",
        PosCommand::ClockOut(_) => "clock_out",
        PosCommand::ReceiveStock(_) => "receive_stock",
        PosCommand::TransferStock(_) => "transfer_stock",
        PosCommand::AdjustStock(_) => "adjust_stock",
    }
}

pub async fn handle_pos_command(
    State(app): State<AppState>,
    Json(envelope): Json<PosRequestEnvelope<PosCommand>>,
) -> Json<PosResponseEnvelope<serde_json::Value>> {
    let operation = pos_operation_label(&envelope.payload);
    let start = Instant::now();
    let span = tracing::info_span!(
        "pos_command",
        idempotency_key = %envelope.idempotency_key,
        store_id = %envelope.store_id,
        register_id = %envelope.register_id,
    );
    let guard = span.enter();

    let response = if envelope.version != ContractVersion::V1_0_0 {
        metrics::counter!(
            POS_COMMANDS_TOTAL,
            1u64,
            "operation" => operation,
            "outcome" => OUTCOME_UNSUPPORTED_VERSION
        );
        Json(PosResponseEnvelope {
            version: ContractVersion::V1_0_0,
            success: false,
            idempotency_key: envelope.idempotency_key,
            payload: None,
            errors: vec![PosError {
                code: "UNSUPPORTED_VERSION".into(),
                message: "Unsupported contract version".into(),
                field: None,
            }],
        })
    } else {
        if let Ok(Some(stored_response)) =
            apex_edge_storage::get_response(&app.pool, envelope.idempotency_key).await
        {
            if let Ok(replayed) =
                serde_json::from_str::<PosResponseEnvelope<serde_json::Value>>(&stored_response)
            {
                metrics::counter!(
                    POS_COMMANDS_TOTAL,
                    1u64,
                    "operation" => operation,
                    "outcome" => OUTCOME_SUCCESS
                );
                metrics::histogram!(
                    POS_COMMAND_DURATION_SECONDS,
                    start.elapsed().as_secs_f64(),
                    "operation" => operation
                );
                drop(guard);
                return Json(replayed);
            }
        }
        let response = crate::pos_handler::execute_pos_command(&app, envelope).await;
        if response.success {
            if let Ok(serialized) = serde_json::to_string(&response) {
                let _ = apex_edge_storage::set_response(
                    &app.pool,
                    response.idempotency_key,
                    &serialized,
                )
                .await;
            }
        }
        metrics::counter!(
            POS_COMMANDS_TOTAL,
            1u64,
            "operation" => operation,
            "outcome" => if response.success { OUTCOME_SUCCESS } else { "error" }
        );
        Json(response)
    };

    metrics::histogram!(
        POS_COMMAND_DURATION_SECONDS,
        start.elapsed().as_secs_f64(),
        "operation" => operation
    );
    drop(guard);
    response
}

/// Read a cart by ID and return its current `CartState`.
/// Returns 404 if the cart does not exist.
pub async fn get_cart_state_handler(
    State(state): State<AppState>,
    Path(cart_id): Path<Uuid>,
) -> Result<Json<CartState>, axum::http::StatusCode> {
    let cart = crate::pos_handler::load_cart_from_db(&state.pool, cart_id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    match cart {
        Some(cart) => {
            let cart_state =
                crate::pos_handler::build_cart_state(&state.pool, state.store_id, &cart).await;
            Ok(Json(cart_state))
        }
        None => Err(axum::http::StatusCode::NOT_FOUND),
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store_id: Uuid,
    pub pool: sqlx::SqlitePool,
    /// When present, GET /metrics returns Prometheus scrape output.
    pub metrics_handle: Option<apex_edge_metrics::PrometheusHandle>,
    pub auth: crate::auth::AuthSettings,
    /// Per-store real-time broadcast hub for POS WebSocket / SSE clients.
    pub stream: crate::stream::StreamHub,
    /// Role of this hub instance (primary or standby) for HA deployments.
    pub role: crate::role::HubRole,
}
