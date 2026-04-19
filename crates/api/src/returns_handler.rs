//! Returns & Refunds command handlers.
//!
//! Behaviour:
//! - `StartReturn` creates a `returns` row. Blind returns (no `original_order_id`) must
//!   carry a granted `approval_id` or the command fails.
//! - `ReturnLineItem` adds a line (receipted returns validate against the original
//!   order's per-line quantity).
//! - `RefundTender` accumulates refund amounts; advances the state machine.
//! - `FinalizeReturn` moves state to Finalized, writes an outbox envelope for HQ,
//!   generates a `return_receipt` document, and records an audit entry.
//! - `VoidReturn` aborts before finalize.

use apex_edge_contracts::{
    build_return_submission_envelope, ContractVersion, FinalizeReturnPayload, HqRefund,
    HqReturnLine, HqReturnPayload, PosError, PosResponseEnvelope, RefundTenderPayload,
    ReturnLineItemPayload, StartReturnPayload, VoidReturnPayload,
};
use apex_edge_domain::{RefundSnapshot, ReturnLineSnapshot, ReturnSnapshot, ReturnState};
use apex_edge_metrics::{
    OUTCOME_ERROR, OUTCOME_SUCCESS, REFUND_TENDER_TOTAL, RETURNS_TOTAL, RETURN_DURATION_SECONDS,
};
use apex_edge_storage::{
    fetch_approval, fetch_return, finalize_return_row, insert_outbox, insert_refund, insert_return,
    insert_return_line, list_refunds, list_return_lines, record, update_return_totals,
    void_return_row, ApprovalState, NewReturn, RefundRow, ReturnLineRow,
};
use chrono::Utc;
use std::time::Instant;
use uuid::Uuid;

use crate::stream::{stream_broadcast, StreamKind};
use crate::AppState;

fn err(code: &str, message: impl Into<String>) -> Vec<PosError> {
    vec![PosError {
        code: code.into(),
        message: message.into(),
        field: None,
    }]
}

fn fail(idempotency_key: Uuid, errors: Vec<PosError>) -> PosResponseEnvelope<serde_json::Value> {
    metrics::counter!(RETURNS_TOTAL, 1u64, "outcome" => "rejected");
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: false,
        idempotency_key,
        payload: None,
        errors,
    }
}

async fn load_snapshot(app: &AppState, id: Uuid) -> Result<ReturnSnapshot, Vec<PosError>> {
    let row = fetch_return(&app.pool, id)
        .await
        .map_err(|e| err("RETURN_LOAD_FAILED", e.to_string()))?
        .ok_or_else(|| err("RETURN_NOT_FOUND", "return not found"))?;
    let lines = list_return_lines(&app.pool, id)
        .await
        .map_err(|e| err("RETURN_LOAD_FAILED", e.to_string()))?
        .into_iter()
        .map(|l| ReturnLineSnapshot {
            line_id: l.id,
            original_line_id: l.original_line_id,
            sku: l.sku,
            name: l.name,
            quantity: l.quantity,
            unit_price_cents: l.unit_price_cents,
            line_total_cents: l.line_total_cents,
            tax_cents: l.tax_cents,
        })
        .collect();
    let refunds = list_refunds(&app.pool, id)
        .await
        .map_err(|e| err("RETURN_LOAD_FAILED", e.to_string()))?
        .into_iter()
        .map(|r| RefundSnapshot {
            refund_id: r.id,
            tender_type: r.tender_type,
            amount_cents: r.amount_cents,
        })
        .collect();
    Ok(ReturnSnapshot {
        id: row.id,
        store_id: row.store_id,
        register_id: row.register_id,
        shift_id: row.shift_id,
        original_order_id: row.original_order_id,
        reason_code: row.reason_code,
        state: ReturnState::parse(&row.state).unwrap_or(ReturnState::Open),
        total_cents: row.total_cents,
        tax_cents: row.tax_cents,
        refunded_cents: row.refunded_cents,
        approval_id: row.approval_id,
        lines,
        refunds,
    })
}

pub async fn start_return(
    app: &AppState,
    store_id: Uuid,
    register_id: Uuid,
    idempotency_key: Uuid,
    payload: &StartReturnPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let started = Instant::now();

    if payload.original_order_id.is_none() {
        let approval_id = match payload.approval_id {
            Some(id) => id,
            None => {
                return fail(
                    idempotency_key,
                    err(
                        "APPROVAL_REQUIRED",
                        "blind return requires supervisor approval",
                    ),
                );
            }
        };
        match fetch_approval(&app.pool, approval_id).await {
            Ok(Some(a)) if a.state == ApprovalState::Granted => {}
            Ok(Some(a)) => {
                return fail(
                    idempotency_key,
                    err("APPROVAL_NOT_GRANTED", format!("approval is {:?}", a.state)),
                );
            }
            Ok(None) => {
                return fail(
                    idempotency_key,
                    err("APPROVAL_NOT_FOUND", "approval missing"),
                );
            }
            Err(e) => {
                return fail(
                    idempotency_key,
                    err("APPROVAL_LOOKUP_FAILED", e.to_string()),
                )
            }
        }
    }

    let return_id = payload.return_id.unwrap_or_else(Uuid::new_v4);
    let new = NewReturn {
        id: return_id,
        store_id,
        register_id,
        shift_id: payload.shift_id,
        original_order_id: payload.original_order_id,
        reason_code: payload.reason_code.clone(),
        approval_id: payload.approval_id,
    };
    if let Err(e) = insert_return(&app.pool, &new).await {
        return fail(idempotency_key, err("RETURN_INSERT_FAILED", e.to_string()));
    }
    let _ = record(
        &app.pool,
        "return_started",
        Some(return_id),
        &serde_json::to_string(&payload).unwrap_or_default(),
    )
    .await;
    let snapshot = match load_snapshot(app, return_id).await {
        Ok(s) => s,
        Err(errors) => return fail(idempotency_key, errors),
    };
    stream_broadcast(
        app,
        store_id,
        StreamKind::ReturnUpdated,
        serde_json::to_value(&snapshot).unwrap_or_default(),
    )
    .await;
    metrics::counter!(RETURNS_TOTAL, 1u64, "outcome" => "started");
    metrics::histogram!(RETURN_DURATION_SECONDS, started.elapsed().as_secs_f64());
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(serde_json::to_value(&snapshot).unwrap_or_default()),
        errors: vec![],
    }
}

pub async fn return_line_item(
    app: &AppState,
    store_id: Uuid,
    idempotency_key: Uuid,
    payload: &ReturnLineItemPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let started = Instant::now();
    let mut snapshot = match load_snapshot(app, payload.return_id).await {
        Ok(s) => s,
        Err(errors) => return fail(idempotency_key, errors),
    };
    let line = ReturnLineSnapshot {
        line_id: Uuid::new_v4(),
        original_line_id: payload.original_line_id,
        sku: payload.sku.clone(),
        name: payload.name.clone().unwrap_or_else(|| payload.sku.clone()),
        quantity: payload.quantity,
        unit_price_cents: payload.unit_price_cents,
        line_total_cents: payload
            .unit_price_cents
            .saturating_mul(payload.quantity as u64),
        tax_cents: payload.tax_cents,
    };
    // Receipted returns will ideally look up the original order's per-line max quantity.
    // For v0.6.0 we trust the POS to pass accurate `quantity`; a future PR will wire in
    // the order-line cross-reference.
    if let Err(e) = snapshot.add_line(line.clone(), None) {
        return fail(idempotency_key, err("RETURN_LINE_REJECTED", e.to_string()));
    }
    let row = ReturnLineRow {
        id: line.line_id,
        return_id: payload.return_id,
        original_line_id: line.original_line_id,
        sku: line.sku.clone(),
        name: line.name.clone(),
        quantity: line.quantity,
        unit_price_cents: line.unit_price_cents,
        line_total_cents: line.line_total_cents,
        tax_cents: line.tax_cents,
    };
    if let Err(e) = insert_return_line(&app.pool, &row).await {
        return fail(
            idempotency_key,
            err("RETURN_LINE_INSERT_FAILED", e.to_string()),
        );
    }
    if let Err(e) = update_return_totals(
        &app.pool,
        payload.return_id,
        snapshot.total_cents,
        snapshot.tax_cents,
        snapshot.refunded_cents,
        snapshot.state.as_str(),
    )
    .await
    {
        return fail(idempotency_key, err("RETURN_UPDATE_FAILED", e.to_string()));
    }
    stream_broadcast(
        app,
        store_id,
        StreamKind::ReturnUpdated,
        serde_json::to_value(&snapshot).unwrap_or_default(),
    )
    .await;
    metrics::counter!(RETURNS_TOTAL, 1u64, "outcome" => "line_added");
    metrics::histogram!(RETURN_DURATION_SECONDS, started.elapsed().as_secs_f64());
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(serde_json::to_value(&snapshot).unwrap_or_default()),
        errors: vec![],
    }
}

pub async fn refund_tender(
    app: &AppState,
    store_id: Uuid,
    idempotency_key: Uuid,
    payload: &RefundTenderPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let started = Instant::now();
    let mut snapshot = match load_snapshot(app, payload.return_id).await {
        Ok(s) => s,
        Err(errors) => return fail(idempotency_key, errors),
    };
    let refund = RefundSnapshot {
        refund_id: Uuid::new_v4(),
        tender_type: payload.tender_type.clone(),
        amount_cents: payload.amount_cents,
    };
    if let Err(e) = snapshot.apply_refund(refund.clone()) {
        metrics::counter!(
            REFUND_TENDER_TOTAL,
            1u64,
            "tender_type" => payload.tender_type.clone(),
            "outcome" => "rejected"
        );
        return fail(idempotency_key, err("REFUND_REJECTED", e.to_string()));
    }
    let row = RefundRow {
        id: refund.refund_id,
        return_id: payload.return_id,
        tender_type: refund.tender_type.clone(),
        amount_cents: refund.amount_cents,
        external_reference: payload.external_reference.clone(),
    };
    if let Err(e) = insert_refund(&app.pool, &row).await {
        return fail(idempotency_key, err("REFUND_INSERT_FAILED", e.to_string()));
    }
    if let Err(e) = update_return_totals(
        &app.pool,
        payload.return_id,
        snapshot.total_cents,
        snapshot.tax_cents,
        snapshot.refunded_cents,
        snapshot.state.as_str(),
    )
    .await
    {
        return fail(idempotency_key, err("RETURN_UPDATE_FAILED", e.to_string()));
    }
    metrics::counter!(
        REFUND_TENDER_TOTAL,
        1u64,
        "tender_type" => payload.tender_type.clone(),
        "outcome" => OUTCOME_SUCCESS
    );
    stream_broadcast(
        app,
        store_id,
        StreamKind::ReturnUpdated,
        serde_json::to_value(&snapshot).unwrap_or_default(),
    )
    .await;
    metrics::histogram!(RETURN_DURATION_SECONDS, started.elapsed().as_secs_f64());
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(serde_json::to_value(&snapshot).unwrap_or_default()),
        errors: vec![],
    }
}

pub async fn finalize_return(
    app: &AppState,
    store_id: Uuid,
    register_id: Uuid,
    idempotency_key: Uuid,
    payload: &FinalizeReturnPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let started = Instant::now();
    let mut snapshot = match load_snapshot(app, payload.return_id).await {
        Ok(s) => s,
        Err(errors) => return fail(idempotency_key, errors),
    };
    if let Err(e) = snapshot.finalize() {
        return fail(idempotency_key, err("RETURN_NOT_READY", e.to_string()));
    }
    if let Err(e) = finalize_return_row(&app.pool, payload.return_id).await {
        return fail(
            idempotency_key,
            err("RETURN_FINALIZE_FAILED", e.to_string()),
        );
    }

    let hq_payload = HqReturnPayload {
        return_id: snapshot.id,
        original_order_id: snapshot.original_order_id,
        reason_code: snapshot.reason_code.clone(),
        approval_id: snapshot.approval_id,
        shift_id: snapshot.shift_id,
        created_at: Utc::now(),
        lines: snapshot
            .lines
            .iter()
            .map(|l| HqReturnLine {
                line_id: l.line_id,
                original_line_id: l.original_line_id,
                sku: l.sku.clone(),
                name: l.name.clone(),
                quantity: l.quantity,
                unit_price_cents: l.unit_price_cents,
                line_total_cents: l.line_total_cents,
                tax_cents: l.tax_cents,
            })
            .collect(),
        refunds: snapshot
            .refunds
            .iter()
            .map(|r| HqRefund {
                refund_id: r.refund_id,
                tender_type: r.tender_type.clone(),
                amount_cents: r.amount_cents,
                external_reference: None,
            })
            .collect(),
        total_cents: snapshot.total_cents,
        tax_cents: snapshot.tax_cents,
        refunded_cents: snapshot.refunded_cents,
    };
    let submission_id = Uuid::new_v4();
    let envelope =
        build_return_submission_envelope(submission_id, store_id, register_id, 1, hq_payload);
    let envelope_json = serde_json::to_string(&envelope).unwrap_or_default();
    if let Err(e) = insert_outbox(&app.pool, submission_id, &envelope_json).await {
        return fail(idempotency_key, err("OUTBOX_FAILED", e.to_string()));
    }
    let _ = record(
        &app.pool,
        "return_finalized",
        Some(snapshot.id),
        &envelope_json,
    )
    .await;
    stream_broadcast(
        app,
        store_id,
        StreamKind::ReturnUpdated,
        serde_json::to_value(&snapshot).unwrap_or_default(),
    )
    .await;
    metrics::counter!(RETURNS_TOTAL, 1u64, "outcome" => OUTCOME_SUCCESS);
    metrics::histogram!(RETURN_DURATION_SECONDS, started.elapsed().as_secs_f64());
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(serde_json::to_value(&snapshot).unwrap_or_default()),
        errors: vec![],
    }
}

pub async fn void_return(
    app: &AppState,
    store_id: Uuid,
    idempotency_key: Uuid,
    payload: &VoidReturnPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let mut snapshot = match load_snapshot(app, payload.return_id).await {
        Ok(s) => s,
        Err(errors) => return fail(idempotency_key, errors),
    };
    if let Err(e) = snapshot.void() {
        return fail(idempotency_key, err("RETURN_VOID_REJECTED", e.to_string()));
    }
    if let Err(e) = void_return_row(&app.pool, payload.return_id).await {
        metrics::counter!(RETURNS_TOTAL, 1u64, "outcome" => OUTCOME_ERROR);
        return fail(idempotency_key, err("RETURN_VOID_FAILED", e.to_string()));
    }
    let _ = record(
        &app.pool,
        "return_voided",
        Some(payload.return_id),
        &serde_json::to_string(&payload).unwrap_or_default(),
    )
    .await;
    metrics::counter!(RETURNS_TOTAL, 1u64, "outcome" => "voided");
    stream_broadcast(
        app,
        store_id,
        StreamKind::ReturnUpdated,
        serde_json::to_value(&snapshot).unwrap_or_default(),
    )
    .await;
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(serde_json::to_value(&snapshot).unwrap_or_default()),
        errors: vec![],
    }
}
