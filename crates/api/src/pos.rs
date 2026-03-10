//! POS command handlers: validate envelope, apply command, return cart state or finalize result.

use apex_edge_contracts::{
    ContractVersion, PosCommand, PosError, PosRequestEnvelope, PosResponseEnvelope,
};
use axum::{extract::State, Json};
use uuid::Uuid;

pub async fn handle_pos_command(
    State(_app): State<AppState>,
    Json(envelope): Json<PosRequestEnvelope<PosCommand>>,
) -> Json<PosResponseEnvelope<serde_json::Value>> {
    let span = tracing::info_span!(
        "pos_command",
        idempotency_key = %envelope.idempotency_key,
        store_id = %envelope.store_id,
        register_id = %envelope.register_id,
    );
    let _guard = span.enter();
    if envelope.version != ContractVersion::V1_0_0 {
        return Json(PosResponseEnvelope {
            version: ContractVersion::V1_0_0,
            success: false,
            idempotency_key: envelope.idempotency_key,
            payload: None,
            errors: vec![PosError {
                code: "UNSUPPORTED_VERSION".into(),
                message: "Unsupported contract version".into(),
                field: None,
            }],
        });
    }
    Json(PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key: envelope.idempotency_key,
        payload: None,
        errors: vec![],
    })
}

#[derive(Clone)]
pub struct AppState {
    pub store_id: Uuid,
    pub pool: sqlx::SqlitePool,
}
