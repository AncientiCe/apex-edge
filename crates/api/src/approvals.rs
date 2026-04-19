//! Supervisor approval REST endpoints.
//!
//! Flows:
//! - `POST /approvals` — create a Pending approval.
//! - `POST /approvals/:id/grant` — supervisor grants.
//! - `POST /approvals/:id/deny` — supervisor denies.
//! - `GET /approvals/:id` — poll state (POS can also subscribe to `/pos/stream`).

use apex_edge_contracts::{
    ApprovalResponse, ApprovalStateDto, DenyApprovalPayload, GrantApprovalPayload,
    RequestApprovalPayload,
};
use apex_edge_metrics::{APPROVALS_TOTAL, APPROVAL_WAIT_DURATION_SECONDS};
use apex_edge_storage::{
    deny_approval, fetch_approval, grant_approval, request_approval, ApprovalRecord, ApprovalState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::stream::{stream_broadcast, StreamKind};
use crate::AppState;

const DEFAULT_TTL_SECONDS: i64 = 300;

fn to_state_dto(s: ApprovalState) -> ApprovalStateDto {
    match s {
        ApprovalState::Pending => ApprovalStateDto::Pending,
        ApprovalState::Granted => ApprovalStateDto::Granted,
        ApprovalState::Denied => ApprovalStateDto::Denied,
        ApprovalState::Expired => ApprovalStateDto::Expired,
    }
}

fn to_response(record: ApprovalRecord) -> ApprovalResponse {
    ApprovalResponse {
        approval_id: record.id,
        action: record.action,
        state: to_state_dto(record.state),
        requested_by: record.requested_by,
        approver_id: record.approver_id,
        decision_reason: record.decision_reason,
        created_at: record.created_at,
        decided_at: record.decided_at,
        expires_at: record.expires_at,
    }
}

pub async fn create_approval(
    State(state): State<AppState>,
    Json(payload): Json<RequestApprovalPayload>,
) -> Result<(StatusCode, Json<ApprovalResponse>), StatusCode> {
    let context_json = payload.context.to_string();
    let ttl = payload.ttl_seconds.unwrap_or(DEFAULT_TTL_SECONDS);
    let record = request_approval(
        &state.pool,
        payload.store_id,
        payload.register_id,
        &payload.action,
        payload.requested_by.as_deref(),
        &context_json,
        ttl,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    metrics::counter!(APPROVALS_TOTAL, 1u64, "action" => record.action.clone(), "outcome" => "requested");

    let response = to_response(record.clone());
    stream_broadcast(
        &state,
        payload.store_id,
        StreamKind::ApprovalRequested,
        serde_json::to_value(&response).unwrap_or_default(),
    )
    .await;
    Ok((StatusCode::ACCEPTED, Json(response)))
}

pub async fn grant_approval_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(payload): Json<GrantApprovalPayload>,
) -> Result<Json<ApprovalResponse>, StatusCode> {
    let existing = fetch_approval(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let record = grant_approval(
        &state.pool,
        id,
        payload.approver_id.as_deref(),
        payload.reason.as_deref(),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let outcome = match record.state {
        ApprovalState::Granted => "granted",
        ApprovalState::Expired => "expired",
        _ => "pending",
    };
    metrics::counter!(
        APPROVALS_TOTAL,
        1u64,
        "action" => record.action.clone(),
        "outcome" => outcome
    );
    let wait = (record.decided_at.unwrap_or(record.created_at) - existing.created_at)
        .num_milliseconds() as f64
        / 1000.0;
    metrics::histogram!(APPROVAL_WAIT_DURATION_SECONDS, wait.max(0.0));

    let response = to_response(record);
    stream_broadcast(
        &state,
        existing.store_id,
        StreamKind::ApprovalDecided,
        serde_json::to_value(&response).unwrap_or_default(),
    )
    .await;
    Ok(Json(response))
}

pub async fn deny_approval_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(payload): Json<DenyApprovalPayload>,
) -> Result<Json<ApprovalResponse>, StatusCode> {
    let existing = fetch_approval(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let record = deny_approval(
        &state.pool,
        id,
        payload.approver_id.as_deref(),
        payload.reason.as_deref(),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    metrics::counter!(
        APPROVALS_TOTAL,
        1u64,
        "action" => record.action.clone(),
        "outcome" => "denied"
    );
    let wait = (record.decided_at.unwrap_or(record.created_at) - existing.created_at)
        .num_milliseconds() as f64
        / 1000.0;
    metrics::histogram!(APPROVAL_WAIT_DURATION_SECONDS, wait.max(0.0));

    let response = to_response(record);
    stream_broadcast(
        &state,
        existing.store_id,
        StreamKind::ApprovalDecided,
        serde_json::to_value(&response).unwrap_or_default(),
    )
    .await;
    Ok(Json(response))
}

pub async fn get_approval_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApprovalResponse>, StatusCode> {
    let record = fetch_approval(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(to_response(record)))
}
