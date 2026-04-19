//! Audit chain verification endpoint (tamper-evidence).

use apex_edge_metrics::{
    AUDIT_CHAIN_LENGTH, AUDIT_CHAIN_VERIFICATIONS_TOTAL, OUTCOME_ERROR, OUTCOME_SUCCESS,
};
use apex_edge_storage::{verify_chain, AuditChainVerification};
use axum::{extract::State, http::StatusCode, Json};

use crate::AppState;

/// GET /audit/verify — re-walk the audit hash chain and report the first breakage, if any.
pub async fn verify_audit_chain(
    State(state): State<AppState>,
) -> Result<Json<AuditChainVerification>, StatusCode> {
    match verify_chain(&state.pool).await {
        Ok(result) => {
            let outcome = if result.ok { OUTCOME_SUCCESS } else { "broken" };
            metrics::counter!(AUDIT_CHAIN_VERIFICATIONS_TOTAL, 1u64, "outcome" => outcome);
            metrics::gauge!(AUDIT_CHAIN_LENGTH, result.checked as f64);
            Ok(Json(result))
        }
        Err(_) => {
            metrics::counter!(AUDIT_CHAIN_VERIFICATIONS_TOTAL, 1u64, "outcome" => OUTCOME_ERROR);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
