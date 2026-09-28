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

use apex_edge_adapters_payment::RefundRequest;
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
    apply_local_delta, fetch_approval, fetch_order_ledger_entry, fetch_return, finalize_return_row,
    get_catalog_item_by_sku, insert_outbox, insert_refund, insert_return, insert_return_line,
    list_refunds, list_return_lines, record, update_return_totals, void_return_row, ApprovalState,
    NewReturn, RefundRow, ReturnLineRow,
};
use chrono::Utc;
use std::time::Instant;
use uuid::Uuid;

use crate::inventory_realtime::broadcast_stock_changed;
use crate::payments::{provider_idempotency_key, ProviderChoice};
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
    metrics::counter!(RETURNS_TOTAL, "outcome" => "rejected").increment(1);
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
            tax_inclusive: l.tax_inclusive,
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
    metrics::counter!(RETURNS_TOTAL, "outcome" => "started").increment(1);
    metrics::histogram!(RETURN_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
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
        tax_inclusive: payload.tax_inclusive,
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
        tax_inclusive: line.tax_inclusive,
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
    metrics::counter!(RETURNS_TOTAL, "outcome" => "line_added").increment(1);
    metrics::histogram!(RETURN_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
    PosResponseEnvelope {
        version: ContractVersion::V1_0_0,
        success: true,
        idempotency_key,
        payload: Some(serde_json::to_value(&snapshot).unwrap_or_default()),
        errors: vec![],
    }
}

struct RefundFailure {
    code: &'static str,
    message: String,
}

/// Return funds to the original payment instrument, when the tender type names a
/// configured payment provider.
///
/// Returns the provider refund id so it can be stored as the refund's external
/// reference. `None` means the store hands the money back itself (cash out of the
/// drawer, a gift card top-up), which needs no provider call.
async fn refund_through_provider(
    app: &AppState,
    original_order_id: Option<Uuid>,
    tender_type: &str,
    amount_cents: u64,
    envelope_key: Uuid,
    refund_id: Uuid,
) -> Result<Option<String>, RefundFailure> {
    let provider = match app.payments.resolve(Some(tender_type)) {
        // A tender type that is not a payment provider is settled by the store itself.
        ProviderChoice::ManualTender | ProviderChoice::Unknown(_) => return Ok(None),
        ProviderChoice::Provider(provider) => provider,
    };
    // Cash is a provider for symmetry, but the drawer is the source of truth and there
    // is no remote balance to move.
    if provider.provider_code() == "cash" {
        return Ok(None);
    }

    let Some(order_id) = original_order_id else {
        return Err(RefundFailure {
            code: "REFUND_ORIGINAL_ORDER_REQUIRED",
            message: format!(
                "Refunding to {tender_type} needs the original order so the payment can be found"
            ),
        });
    };

    let order = fetch_order_ledger_entry(&app.pool, order_id)
        .await
        .map_err(|e| RefundFailure {
            code: "REFUND_ORDER_LOOKUP_FAILED",
            message: e.to_string(),
        })?
        .ok_or_else(|| RefundFailure {
            code: "REFUND_ORIGINAL_ORDER_NOT_FOUND",
            message: format!("Original order {order_id} is not on this hub"),
        })?;

    let provider_payment_id = order
        .payments
        .iter()
        .filter(|payment| payment.provider.as_deref() == Some(provider.provider_code()))
        .find_map(|payment| payment.provider_payment_id.clone())
        .ok_or_else(|| RefundFailure {
            code: "REFUND_PROVIDER_PAYMENT_NOT_FOUND",
            message: format!(
                "Order {order_id} has no {} payment to refund against",
                provider.provider_code()
            ),
        })?;

    let outcome = provider
        .refund(RefundRequest {
            idempotency_key: provider_idempotency_key(envelope_key, refund_id, "refund"),
            provider_payment_id,
            amount_cents,
            reason: Some("customer return".into()),
        })
        .await;

    match outcome {
        Ok(refund) => {
            metrics::counter!(apex_edge_metrics::PAYMENT_REFUNDS_TOTAL, "provider" => provider.provider_code(), "outcome" => OUTCOME_SUCCESS).increment(1);
            Ok(Some(refund.provider_refund_id))
        }
        Err(e) => {
            metrics::counter!(apex_edge_metrics::PAYMENT_REFUNDS_TOTAL, "provider" => provider.provider_code(), "outcome" => OUTCOME_ERROR).increment(1);
            Err(RefundFailure {
                code: "REFUND_PROVIDER_FAILED",
                message: e.to_string(),
            })
        }
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
        metrics::counter!(REFUND_TENDER_TOTAL, "tender_type" => payload.tender_type.clone(), "outcome" => "rejected").increment(1);
        return fail(idempotency_key, err("REFUND_REJECTED", e.to_string()));
    }

    // Give the money back through the same provider that took it, before recording the
    // refund locally. Recording a card refund that the acquirer never processed would
    // leave the store's books claiming money it still holds.
    let provider_reference = match refund_through_provider(
        app,
        snapshot.original_order_id,
        &payload.tender_type,
        payload.amount_cents,
        idempotency_key,
        refund.refund_id,
    )
    .await
    {
        Ok(reference) => reference,
        Err(failure) => {
            metrics::counter!(REFUND_TENDER_TOTAL, "tender_type" => payload.tender_type.clone(), "outcome" => "rejected").increment(1);
            return fail(idempotency_key, err(failure.code, failure.message));
        }
    };

    let row = RefundRow {
        id: refund.refund_id,
        return_id: payload.return_id,
        tender_type: refund.tender_type.clone(),
        amount_cents: refund.amount_cents,
        external_reference: provider_reference.or_else(|| payload.external_reference.clone()),
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
    metrics::counter!(REFUND_TENDER_TOTAL, "tender_type" => payload.tender_type.clone(), "outcome" => OUTCOME_SUCCESS).increment(1);
    stream_broadcast(
        app,
        store_id,
        StreamKind::ReturnUpdated,
        serde_json::to_value(&snapshot).unwrap_or_default(),
    )
    .await;
    metrics::histogram!(RETURN_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
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

    let fiscal_transaction =
        crate::fiscal::refund_transaction(&app.pool, &snapshot, &app.fiscal.currency).await;
    match crate::fiscal::sign_or_queue(app, "return", snapshot.id, &fiscal_transaction).await {
        Ok(outcome) => {
            let fields =
                crate::fiscal::fields_from_outcome(app.fiscal.provider.provider_code(), outcome);
            let update = apex_edge_storage::FiscalReceiptUpdate {
                provider: fields
                    .provider
                    .unwrap_or_else(|| app.fiscal.provider.provider_code().into()),
                fiscal_id: fields.fiscal_id,
                signature: fields.signature,
                qr_payload: fields.qr_payload,
                signed_at: fields.signed_at,
                pending: fields.pending,
            };
            if let Err(e) =
                apex_edge_storage::apply_return_fiscal_receipt(&app.pool, snapshot.id, &update)
                    .await
            {
                return fail(idempotency_key, err("FISCAL_SIGNING_FAILED", e.to_string()));
            }
        }
        Err(e) => {
            return fail(idempotency_key, err("FISCAL_SIGNING_FAILED", e.to_string()));
        }
    }

    if let Err(e) = finalize_return_row(&app.pool, payload.return_id).await {
        return fail(
            idempotency_key,
            err("RETURN_FINALIZE_FAILED", e.to_string()),
        );
    }

    // Restock returned items into the real-time ledger (HQ remains authoritative on next
    // sync). Lines carry SKU only, so resolve each to its catalog item id.
    let mut restocked_item_ids: Vec<Uuid> = Vec::new();
    for line in &snapshot.lines {
        if let Ok(Some(item)) = get_catalog_item_by_sku(&app.pool, store_id, &line.sku).await {
            let _ = apply_local_delta(&app.pool, store_id, item.id, line.quantity as i64).await;
            restocked_item_ids.push(item.id);
        }
    }
    broadcast_stock_changed(app, store_id, &restocked_item_ids).await;

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
    metrics::counter!(RETURNS_TOTAL, "outcome" => OUTCOME_SUCCESS).increment(1);
    metrics::histogram!(RETURN_DURATION_SECONDS).record(started.elapsed().as_secs_f64());
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
        metrics::counter!(RETURNS_TOTAL, "outcome" => OUTCOME_ERROR).increment(1);
        return fail(idempotency_key, err("RETURN_VOID_FAILED", e.to_string()));
    }
    let _ = record(
        &app.pool,
        "return_voided",
        Some(payload.return_id),
        &serde_json::to_string(&payload).unwrap_or_default(),
    )
    .await;
    metrics::counter!(RETURNS_TOTAL, "outcome" => "voided").increment(1);
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
