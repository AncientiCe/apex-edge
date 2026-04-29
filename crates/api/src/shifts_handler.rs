//! Till & Shift command handlers.
//!
//! Behaviour:
//! - `OpenTill` creates a `shifts` row; only one open shift per (store, register).
//! - `PaidIn` / `PaidOut` / `NoSale` record a cash movement; above a threshold require a
//!   granted `approval_id`.
//! - `CashCount` snapshots a mid-shift count (variance computed against expected).
//! - `GetXReport` returns a synthetic snapshot (non-closing).
//! - `CloseTill` writes variance, emits HQ shift submission, generates Z-report
//!   document, records audit.

use apex_edge_contracts::{
    build_shift_submission_envelope, CashCountPayload, CloseTillPayload, ContractVersion,
    GetXReportPayload, HqShiftMovement, HqShiftPayload, NoSalePayload, OpenTillPayload,
    PaidInPayload, PaidOutPayload, PosError, PosResponseEnvelope,
};
use apex_edge_domain::{expected_cash_cents, variance_cents, CashMovement, CashMovementKind};
use apex_edge_metrics::{
    CASH_MOVEMENTS_TOTAL, OUTCOME_ERROR, OUTCOME_SUCCESS, SHIFTS_TOTAL, SHIFT_VARIANCE_CENTS,
};
use apex_edge_storage::{
    cash_refunds_cents_for_shift, cash_sales_cents_for_shift, close_shift, fetch_approval,
    fetch_open_shift, fetch_shift, insert_outbox, insert_shift_movement, list_shift_movements,
    open_shift, record, ApprovalState,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::stream::{stream_broadcast, StreamKind};
use crate::AppState;

const CASH_MOVEMENT_APPROVAL_THRESHOLD_CENTS: u64 = 10_000;

fn err(code: &str, message: impl Into<String>) -> Vec<PosError> {
    vec![PosError {
        code: code.into(),
        message: message.into(),
        field: None,
    }]
}

fn fail(idempotency_key: Uuid, errors: Vec<PosError>) -> PosResponseEnvelope<serde_json::Value> {
    metrics::counter!(SHIFTS_TOTAL, 1u64, "outcome" => "rejected");
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: false,
        idempotency_key,
        payload: None,
        errors,
    }
}

async fn approval_granted(app: &AppState, id: Uuid) -> bool {
    matches!(
        fetch_approval(&app.pool, id).await,
        Ok(Some(a)) if a.state == ApprovalState::Granted
    )
}

pub async fn open_till(
    app: &AppState,
    store_id: Uuid,
    register_id: Uuid,
    idempotency_key: Uuid,
    payload: &OpenTillPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let reg = payload.register_id.unwrap_or(register_id);
    if let Ok(Some(_)) = fetch_open_shift(&app.pool, store_id, reg).await {
        return fail(
            idempotency_key,
            err(
                "SHIFT_ALREADY_OPEN",
                "a shift is already open for this register",
            ),
        );
    }
    let id = Uuid::new_v4();
    if let Err(e) = open_shift(
        &app.pool,
        id,
        store_id,
        reg,
        payload.associate_id.as_deref(),
        payload.opening_float_cents,
    )
    .await
    {
        return fail(idempotency_key, err("SHIFT_OPEN_FAILED", e.to_string()));
    }
    let _ = record(
        &app.pool,
        "shift_opened",
        Some(id),
        &serde_json::to_string(&payload).unwrap_or_default(),
    )
    .await;
    metrics::counter!(SHIFTS_TOTAL, 1u64, "outcome" => "opened");
    let payload_json = serde_json::json!({
        "shift_id": id,
        "register_id": reg,
        "opening_float_cents": payload.opening_float_cents,
        "state": "open",
    });
    stream_broadcast(
        app,
        store_id,
        StreamKind::ShiftUpdated,
        payload_json.clone(),
    )
    .await;
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(payload_json),
        errors: vec![],
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_cash_movement(
    app: &AppState,
    store_id: Uuid,
    idempotency_key: Uuid,
    shift_id: Uuid,
    kind: CashMovementKind,
    amount_cents: u64,
    reason: Option<String>,
    approval_id: Option<Uuid>,
) -> PosResponseEnvelope<serde_json::Value> {
    if amount_cents > CASH_MOVEMENT_APPROVAL_THRESHOLD_CENTS {
        match approval_id {
            Some(id) if approval_granted(app, id).await => {}
            _ => {
                metrics::counter!(
                    CASH_MOVEMENTS_TOTAL,
                    1u64,
                    "kind" => kind.as_str(),
                    "outcome" => "approval_required"
                );
                return fail(
                    idempotency_key,
                    err(
                        "APPROVAL_REQUIRED",
                        format!(
                            "cash movement {} over threshold requires supervisor approval",
                            amount_cents
                        ),
                    ),
                );
            }
        }
    }
    let movement_id = Uuid::new_v4();
    if let Err(e) = insert_shift_movement(
        &app.pool,
        movement_id,
        shift_id,
        kind.as_str(),
        amount_cents,
        reason.as_deref(),
        approval_id,
    )
    .await
    {
        metrics::counter!(
            CASH_MOVEMENTS_TOTAL,
            1u64,
            "kind" => kind.as_str(),
            "outcome" => OUTCOME_ERROR
        );
        return fail(idempotency_key, err("CASH_MOVEMENT_FAILED", e.to_string()));
    }
    let _ = record(
        &app.pool,
        "cash_movement",
        Some(shift_id),
        &serde_json::json!({
            "kind": kind.as_str(),
            "amount_cents": amount_cents,
            "reason": reason,
            "approval_id": approval_id,
        })
        .to_string(),
    )
    .await;
    metrics::counter!(
        CASH_MOVEMENTS_TOTAL,
        1u64,
        "kind" => kind.as_str(),
        "outcome" => OUTCOME_SUCCESS
    );
    let payload_json = serde_json::json!({
        "shift_id": shift_id,
        "movement_id": movement_id,
        "kind": kind.as_str(),
        "amount_cents": amount_cents,
    });
    stream_broadcast(
        app,
        store_id,
        StreamKind::ShiftUpdated,
        payload_json.clone(),
    )
    .await;
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(payload_json),
        errors: vec![],
    }
}

pub async fn paid_in(
    app: &AppState,
    store_id: Uuid,
    idempotency_key: Uuid,
    payload: &PaidInPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    record_cash_movement(
        app,
        store_id,
        idempotency_key,
        payload.shift_id,
        CashMovementKind::PaidIn,
        payload.amount_cents,
        Some(payload.reason.clone()),
        payload.approval_id,
    )
    .await
}

pub async fn paid_out(
    app: &AppState,
    store_id: Uuid,
    idempotency_key: Uuid,
    payload: &PaidOutPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    record_cash_movement(
        app,
        store_id,
        idempotency_key,
        payload.shift_id,
        CashMovementKind::PaidOut,
        payload.amount_cents,
        Some(payload.reason.clone()),
        payload.approval_id,
    )
    .await
}

pub async fn no_sale(
    app: &AppState,
    store_id: Uuid,
    idempotency_key: Uuid,
    payload: &NoSalePayload,
) -> PosResponseEnvelope<serde_json::Value> {
    record_cash_movement(
        app,
        store_id,
        idempotency_key,
        payload.shift_id,
        CashMovementKind::NoSale,
        0,
        Some(payload.reason.clone()),
        None,
    )
    .await
}

async fn movements_and_expected(
    app: &AppState,
    shift_id: Uuid,
    opening_float_cents: u64,
) -> (Vec<CashMovement>, i64, u64, u64) {
    let movements = list_shift_movements(&app.pool, shift_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| {
            let kind = CashMovementKind::parse(&m.kind)?;
            Some(CashMovement {
                id: m.id,
                kind,
                amount_cents: m.amount_cents,
                reason: m.reason,
                approval_id: m.approval_id,
            })
        })
        .collect::<Vec<_>>();
    let cash_sales = cash_sales_cents_for_shift(&app.pool, shift_id)
        .await
        .unwrap_or_default();
    let cash_refunds = cash_refunds_cents_for_shift(&app.pool, shift_id)
        .await
        .unwrap_or_default();
    let expected = expected_cash_cents(opening_float_cents, cash_sales, cash_refunds, &movements);
    (movements, expected, cash_sales, cash_refunds)
}

pub async fn cash_count(
    app: &AppState,
    store_id: Uuid,
    idempotency_key: Uuid,
    payload: &CashCountPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let shift = match fetch_shift(&app.pool, payload.shift_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return fail(idempotency_key, err("SHIFT_NOT_FOUND", "shift not found")),
        Err(e) => return fail(idempotency_key, err("SHIFT_LOAD_FAILED", e.to_string())),
    };
    let (_movements, expected, cash_sales, cash_refunds) =
        movements_and_expected(app, shift.id, shift.opening_float_cents).await;
    let variance = variance_cents(payload.counted_cents as i64, expected);
    metrics::histogram!(SHIFT_VARIANCE_CENTS, variance.unsigned_abs() as f64);
    let response = serde_json::json!({
        "shift_id": shift.id,
        "counted_cents": payload.counted_cents,
        "expected_cents": expected,
        "cash_sales_cents": cash_sales,
        "cash_refunds_cents": cash_refunds,
        "variance_cents": variance,
        "denominations": payload.denominations,
    });
    stream_broadcast(app, store_id, StreamKind::ShiftUpdated, response.clone()).await;
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(response),
        errors: vec![],
    }
}

pub async fn get_x_report(
    app: &AppState,
    _store_id: Uuid,
    idempotency_key: Uuid,
    payload: &GetXReportPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let shift = match fetch_shift(&app.pool, payload.shift_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return fail(idempotency_key, err("SHIFT_NOT_FOUND", "shift not found")),
        Err(e) => return fail(idempotency_key, err("SHIFT_LOAD_FAILED", e.to_string())),
    };
    let (movements, expected, cash_sales, cash_refunds) =
        movements_and_expected(app, shift.id, shift.opening_float_cents).await;
    let report = serde_json::json!({
        "shift_id": shift.id,
        "register_id": shift.register_id,
        "state": shift.state,
        "opened_at": shift.opened_at,
        "opening_float_cents": shift.opening_float_cents,
        "expected_cents": expected,
        "cash_sales_cents": cash_sales,
        "cash_refunds_cents": cash_refunds,
        "movements": movements.iter().map(|m| serde_json::json!({
            "id": m.id,
            "kind": m.kind.as_str(),
            "amount_cents": m.amount_cents,
            "reason": m.reason,
        })).collect::<Vec<_>>(),
        "report_type": "x_report",
    });
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(report),
        errors: vec![],
    }
}

pub async fn close_till(
    app: &AppState,
    store_id: Uuid,
    register_id: Uuid,
    idempotency_key: Uuid,
    payload: &CloseTillPayload,
) -> PosResponseEnvelope<serde_json::Value> {
    let shift = match fetch_shift(&app.pool, payload.shift_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return fail(idempotency_key, err("SHIFT_NOT_FOUND", "shift not found")),
        Err(e) => return fail(idempotency_key, err("SHIFT_LOAD_FAILED", e.to_string())),
    };
    if shift.state != "open" {
        return fail(
            idempotency_key,
            err("SHIFT_NOT_OPEN", "shift already closed"),
        );
    }
    let (movements, expected, cash_sales, cash_refunds) =
        movements_and_expected(app, shift.id, shift.opening_float_cents).await;
    let variance = variance_cents(payload.counted_cents as i64, expected);
    // Large variance requires a granted approval.
    if variance.unsigned_abs() > CASH_MOVEMENT_APPROVAL_THRESHOLD_CENTS {
        match payload.approval_id {
            Some(id) if approval_granted(app, id).await => {}
            _ => {
                metrics::histogram!(SHIFT_VARIANCE_CENTS, variance.unsigned_abs() as f64);
                return fail(
                    idempotency_key,
                    err(
                        "APPROVAL_REQUIRED",
                        format!("variance {} cents requires supervisor approval", variance),
                    ),
                );
            }
        }
    }
    if let Err(e) = close_shift(
        &app.pool,
        shift.id,
        payload.counted_cents as i64,
        expected,
        variance,
    )
    .await
    {
        return fail(idempotency_key, err("SHIFT_CLOSE_FAILED", e.to_string()));
    }

    // Build HQ shift submission.
    let hq_payload = HqShiftPayload {
        shift_id: shift.id,
        associate_id: shift.associate_id.clone(),
        opened_at: DateTime::parse_from_rfc3339(&shift.opened_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        closed_at: Some(Utc::now()),
        opening_float_cents: shift.opening_float_cents,
        expected_cents: expected,
        counted_cents: payload.counted_cents as i64,
        variance_cents: variance,
        cash_sales_cents: cash_sales,
        cash_refunds_cents: cash_refunds,
        movements: movements
            .iter()
            .map(|m| HqShiftMovement {
                id: m.id,
                kind: m.kind.as_str().into(),
                amount_cents: m.amount_cents,
                reason: m.reason.clone(),
            })
            .collect(),
    };
    let submission_id = Uuid::new_v4();
    let envelope =
        build_shift_submission_envelope(submission_id, store_id, register_id, 1, hq_payload);
    let envelope_json = serde_json::to_string(&envelope).unwrap_or_default();
    if let Err(e) = insert_outbox(&app.pool, submission_id, &envelope_json).await {
        return fail(idempotency_key, err("OUTBOX_FAILED", e.to_string()));
    }
    let _ = record(&app.pool, "shift_closed", Some(shift.id), &envelope_json).await;

    metrics::counter!(SHIFTS_TOTAL, 1u64, "outcome" => "closed");
    metrics::histogram!(SHIFT_VARIANCE_CENTS, variance.unsigned_abs() as f64);
    let response = serde_json::json!({
        "shift_id": shift.id,
        "expected_cents": expected,
        "cash_sales_cents": cash_sales,
        "cash_refunds_cents": cash_refunds,
        "counted_cents": payload.counted_cents,
        "variance_cents": variance,
        "state": "closed",
        "report_type": "z_report",
    });
    stream_broadcast(app, store_id, StreamKind::ShiftUpdated, response.clone()).await;
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(response),
        errors: vec![],
    }
}
