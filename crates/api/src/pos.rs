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
        let response = crate::pos_handler::execute_pos_command(&app, envelope).await;
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
        "operation" => operation,
        "span_entered" => std::mem::size_of_val(&guard).to_string()
    );
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
}
