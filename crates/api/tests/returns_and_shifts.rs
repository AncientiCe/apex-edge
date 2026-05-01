//! End-to-end behaviour for Returns & Refunds and Till & Shift POS commands.
//!
//! These tests drive commands through `execute_pos_command`, asserting state machine,
//! approval gating, HQ envelope generation (outbox), and audit chain growth.

use apex_edge_api::{pos_handler::execute_pos_command, AppState, AuthSettings, HubRole, StreamHub};
use apex_edge_contracts::{
    CashCountPayload, CloseTillPayload, ContractVersion, FinalizeReturnPayload, NoSalePayload,
    OpenTillPayload, PaidInPayload, PaidOutPayload, PosCommand, PosRequestEnvelope,
    RefundTenderPayload, ReturnLineItemPayload, StartReturnPayload, VoidReturnPayload,
};
use apex_edge_storage::{
    create_sqlite_pool, finalize_return_row, grant_approval, insert_order_ledger_entry,
    insert_refund, insert_return, request_approval, run_migrations, set_audit_key,
    update_return_totals, ApprovalState, AuditKey, NewOrderLedgerEntry, NewOrderLineEntry,
    NewOrderPaymentEntry, NewReturn, RefundRow,
};
use sqlx::Row;
use std::collections::BTreeMap;
use uuid::Uuid;

fn state_for(pool: sqlx::SqlitePool) -> AppState {
    AppState {
        store_id: Uuid::nil(),
        pool,
        metrics_handle: None,
        auth: AuthSettings::default(),
        stream: StreamHub::new(),
        role: HubRole::Primary,
    }
}

async fn setup() -> AppState {
    set_audit_key(AuditKey::new("test-hub", b"test-secret".to_vec()));
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    state_for(pool)
}

fn env<T>(store_id: Uuid, register_id: Uuid, payload: T) -> PosRequestEnvelope<T> {
    PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id,
        register_id,
        payload,
    }
}

#[tokio::test]
async fn blind_return_requires_approval_and_succeeds_when_granted() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    // Without approval: fails.
    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::StartReturn(StartReturnPayload {
                return_id: None,
                original_order_id: None,
                reason_code: Some("no_receipt".into()),
                approval_id: None,
                shift_id: None,
            }),
        ),
    )
    .await;
    assert!(!resp.success);
    assert_eq!(resp.errors[0].code, "APPROVAL_REQUIRED");

    // With a granted approval: succeeds.
    let approval = request_approval(
        &state.pool,
        store,
        Some(register),
        "blind_return",
        None,
        "{}",
        300,
    )
    .await
    .unwrap();
    let granted = grant_approval(&state.pool, approval.id, Some("mgr"), None)
        .await
        .unwrap();
    assert_eq!(granted.state, ApprovalState::Granted);

    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::StartReturn(StartReturnPayload {
                return_id: None,
                original_order_id: None,
                reason_code: Some("no_receipt".into()),
                approval_id: Some(approval.id),
                shift_id: None,
            }),
        ),
    )
    .await;
    assert!(resp.success, "errors: {:?}", resp.errors);
}

#[tokio::test]
async fn full_return_flow_updates_outbox_and_audit() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    let start = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::StartReturn(StartReturnPayload {
                return_id: None,
                original_order_id: Some(Uuid::new_v4()),
                reason_code: Some("damaged".into()),
                approval_id: None,
                shift_id: None,
            }),
        ),
    )
    .await;
    assert!(start.success);
    let return_id: Uuid =
        serde_json::from_value(start.payload.as_ref().unwrap().get("id").cloned().unwrap())
            .unwrap();

    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::ReturnLineItem(ReturnLineItemPayload {
                return_id,
                sku: "SKU-1".into(),
                name: Some("Widget".into()),
                quantity: 2,
                unit_price_cents: 1000,
                tax_cents: 200,
                original_line_id: None,
            }),
        ),
    )
    .await;
    assert!(resp.success);
    assert_eq!(resp.payload.as_ref().unwrap()["total_cents"], 2200);

    // Partial then full refund.
    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::RefundTender(RefundTenderPayload {
                return_id,
                tender_type: "cash".into(),
                amount_cents: 1000,
                external_reference: None,
            }),
        ),
    )
    .await;
    assert!(resp.success);
    assert_eq!(resp.payload.as_ref().unwrap()["state"], "tendered");

    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::RefundTender(RefundTenderPayload {
                return_id,
                tender_type: "cash".into(),
                amount_cents: 1200,
                external_reference: None,
            }),
        ),
    )
    .await;
    assert!(resp.success);
    assert_eq!(resp.payload.as_ref().unwrap()["state"], "paid");

    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::FinalizeReturn(FinalizeReturnPayload { return_id }),
        ),
    )
    .await;
    assert!(resp.success);
    assert_eq!(resp.payload.as_ref().unwrap()["state"], "finalized");

    // Outbox has a return_submission envelope.
    let outbox: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(outbox >= 1, "outbox should have a submission");

    // Audit chain has entries and verifies clean.
    let v = apex_edge_storage::verify_chain(&state.pool).await.unwrap();
    assert!(v.ok, "audit chain must verify clean after return flow");
    assert!(v.checked >= 2);
}

#[tokio::test]
async fn over_refund_is_rejected() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    let start = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::StartReturn(StartReturnPayload {
                return_id: None,
                original_order_id: Some(Uuid::new_v4()),
                reason_code: None,
                approval_id: None,
                shift_id: None,
            }),
        ),
    )
    .await;
    let return_id: Uuid =
        serde_json::from_value(start.payload.unwrap().get("id").cloned().unwrap()).unwrap();

    execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::ReturnLineItem(ReturnLineItemPayload {
                return_id,
                sku: "SKU-1".into(),
                name: None,
                quantity: 1,
                unit_price_cents: 1000,
                tax_cents: 0,
                original_line_id: None,
            }),
        ),
    )
    .await;

    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::RefundTender(RefundTenderPayload {
                return_id,
                tender_type: "cash".into(),
                amount_cents: 5000,
                external_reference: None,
            }),
        ),
    )
    .await;
    assert!(!resp.success);
    assert_eq!(resp.errors[0].code, "REFUND_REJECTED");
}

#[tokio::test]
async fn void_return_before_finalize_marks_voided() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    let start = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::StartReturn(StartReturnPayload {
                return_id: None,
                original_order_id: Some(Uuid::new_v4()),
                reason_code: None,
                approval_id: None,
                shift_id: None,
            }),
        ),
    )
    .await;
    let return_id: Uuid =
        serde_json::from_value(start.payload.unwrap().get("id").cloned().unwrap()).unwrap();

    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::VoidReturn(VoidReturnPayload {
                return_id,
                reason: None,
            }),
        ),
    )
    .await;
    assert!(resp.success);
    assert_eq!(resp.payload.unwrap()["state"], "voided");
}

#[tokio::test]
async fn cannot_open_two_shifts_on_same_register() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    let first = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::OpenTill(OpenTillPayload {
                register_id: Some(register),
                associate_id: Some("a1".into()),
                opening_float_cents: 10_000,
            }),
        ),
    )
    .await;
    assert!(first.success);

    let second = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::OpenTill(OpenTillPayload {
                register_id: Some(register),
                associate_id: Some("a2".into()),
                opening_float_cents: 20_000,
            }),
        ),
    )
    .await;
    assert!(!second.success);
    assert_eq!(second.errors[0].code, "SHIFT_ALREADY_OPEN");
}

#[tokio::test]
async fn paid_out_over_threshold_requires_approval() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    let opened = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::OpenTill(OpenTillPayload {
                register_id: Some(register),
                associate_id: None,
                opening_float_cents: 10_000,
            }),
        ),
    )
    .await;
    let shift_id: Uuid =
        serde_json::from_value(opened.payload.unwrap().get("shift_id").cloned().unwrap()).unwrap();

    // Small paid_out: OK.
    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::PaidOut(PaidOutPayload {
                shift_id,
                amount_cents: 500,
                reason: "supplies".into(),
                approval_id: None,
            }),
        ),
    )
    .await;
    assert!(resp.success);

    // Large paid_out without approval: rejected.
    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::PaidOut(PaidOutPayload {
                shift_id,
                amount_cents: 50_000,
                reason: "vendor".into(),
                approval_id: None,
            }),
        ),
    )
    .await;
    assert!(!resp.success);
    assert_eq!(resp.errors[0].code, "APPROVAL_REQUIRED");
}

#[tokio::test]
async fn no_sale_records_zero_amount_movement() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    let opened = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::OpenTill(OpenTillPayload {
                register_id: Some(register),
                associate_id: None,
                opening_float_cents: 0,
            }),
        ),
    )
    .await;
    let shift_id: Uuid =
        serde_json::from_value(opened.payload.unwrap().get("shift_id").cloned().unwrap()).unwrap();

    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::NoSale(NoSalePayload {
                shift_id,
                reason: "curious".into(),
            }),
        ),
    )
    .await;
    assert!(resp.success);
}

#[tokio::test]
async fn close_till_with_matching_count_succeeds_and_generates_outbox() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    let opened = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::OpenTill(OpenTillPayload {
                register_id: Some(register),
                associate_id: None,
                opening_float_cents: 5_000,
            }),
        ),
    )
    .await;
    let shift_id: Uuid =
        serde_json::from_value(opened.payload.unwrap().get("shift_id").cloned().unwrap()).unwrap();

    execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::PaidIn(PaidInPayload {
                shift_id,
                amount_cents: 1_000,
                reason: "tip jar".into(),
                approval_id: None,
            }),
        ),
    )
    .await;

    // Expected = 5_000 + 1_000 = 6_000.
    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::CloseTill(CloseTillPayload {
                shift_id,
                counted_cents: 6_000,
                approval_id: None,
            }),
        ),
    )
    .await;
    assert!(resp.success, "close failed: {:?}", resp.errors);
    assert_eq!(resp.payload.as_ref().unwrap()["variance_cents"], 0);
    assert_eq!(resp.payload.as_ref().unwrap()["state"], "closed");

    let outbox: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(outbox >= 1);
}

#[tokio::test]
async fn x_report_and_close_till_include_ledger_cash_sales_and_refunds() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    let opened = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::OpenTill(OpenTillPayload {
                register_id: Some(register),
                associate_id: None,
                opening_float_cents: 5_000,
            }),
        ),
    )
    .await;
    let shift_id: Uuid =
        serde_json::from_value(opened.payload.unwrap().get("shift_id").cloned().unwrap()).unwrap();

    insert_order_ledger_entry(
        &state.pool,
        &NewOrderLedgerEntry {
            order_id: Uuid::new_v4(),
            cart_id: Uuid::new_v4(),
            store_id: store,
            register_id: register,
            shift_id: Some(shift_id),
            subtotal_cents: 1_500,
            discount_cents: 0,
            tax_cents: 0,
            total_cents: 1_500,
            submission_id: Some(Uuid::new_v4()),
            lines: vec![NewOrderLineEntry {
                line_id: Uuid::new_v4(),
                item_id: Uuid::new_v4(),
                sku: "SHIFT-CASH".into(),
                name: "Shift Cash Item".into(),
                quantity: 1,
                unit_price_cents: 1_500,
                line_total_cents: 1_500,
                discount_cents: 0,
                tax_cents: 0,
            }],
            payments: vec![NewOrderPaymentEntry {
                tender_id: Uuid::new_v4(),
                tender_type: "cash".into(),
                amount_cents: 1_500,
                tip_amount_cents: 0,
                external_reference: Some("cash".into()),
                provider: None,
                provider_payment_id: None,
                entry_method: None,
            }],
        },
    )
    .await
    .expect("insert cash sale");

    let return_id = Uuid::new_v4();
    insert_return(
        &state.pool,
        &NewReturn {
            id: return_id,
            store_id: store,
            register_id: register,
            shift_id: Some(shift_id),
            original_order_id: None,
            reason_code: Some("cash_refund".into()),
            approval_id: None,
        },
    )
    .await
    .expect("insert return");
    insert_refund(
        &state.pool,
        &RefundRow {
            id: Uuid::new_v4(),
            return_id,
            tender_type: "cash".into(),
            amount_cents: 400,
            external_reference: None,
        },
    )
    .await
    .expect("insert refund");
    update_return_totals(&state.pool, return_id, 400, 0, 400, "paid")
        .await
        .expect("update return totals");
    finalize_return_row(&state.pool, return_id)
        .await
        .expect("finalize return row");

    execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::PaidIn(PaidInPayload {
                shift_id,
                amount_cents: 1_000,
                reason: "float top-up".into(),
                approval_id: None,
            }),
        ),
    )
    .await;
    execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::PaidOut(PaidOutPayload {
                shift_id,
                amount_cents: 250,
                reason: "petty cash".into(),
                approval_id: None,
            }),
        ),
    )
    .await;

    let x_report = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::GetXReport(apex_edge_contracts::GetXReportPayload { shift_id }),
        ),
    )
    .await;
    assert!(x_report.success);
    let payload = x_report.payload.as_ref().unwrap();
    assert_eq!(payload["cash_sales_cents"], 1_500);
    assert_eq!(payload["cash_refunds_cents"], 400);
    assert_eq!(payload["expected_cents"], 6_850);

    let closed = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::CloseTill(CloseTillPayload {
                shift_id,
                counted_cents: 6_850,
                approval_id: None,
            }),
        ),
    )
    .await;
    assert!(closed.success, "close failed: {:?}", closed.errors);
    let payload = closed.payload.as_ref().unwrap();
    assert_eq!(payload["expected_cents"], 6_850);
    assert_eq!(payload["cash_sales_cents"], 1_500);
    assert_eq!(payload["cash_refunds_cents"], 400);
    assert_eq!(payload["variance_cents"], 0);
}

#[tokio::test]
async fn cash_count_reports_variance_without_closing() {
    let state = setup().await;
    let store = Uuid::new_v4();
    let register = Uuid::new_v4();

    let opened = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::OpenTill(OpenTillPayload {
                register_id: Some(register),
                associate_id: None,
                opening_float_cents: 5_000,
            }),
        ),
    )
    .await;
    let shift_id: Uuid =
        serde_json::from_value(opened.payload.unwrap().get("shift_id").cloned().unwrap()).unwrap();

    let mut denoms = BTreeMap::new();
    denoms.insert("100".into(), 45);

    let resp = execute_pos_command(
        &state,
        env(
            store,
            register,
            PosCommand::CashCount(CashCountPayload {
                shift_id,
                counted_cents: 4_500,
                denominations: denoms,
            }),
        ),
    )
    .await;
    assert!(resp.success);
    let payload = resp.payload.unwrap();
    assert_eq!(payload["expected_cents"], 5_000);
    assert_eq!(payload["variance_cents"], -500);

    // Shift remains open after a mid-shift count.
    let row = sqlx::query("SELECT state FROM shifts WHERE id = ?")
        .bind(shift_id.to_string())
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let state_str: String = row.get("state");
    assert_eq!(state_str, "open");
}
