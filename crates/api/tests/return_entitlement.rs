use apex_edge_api::{pos_handler::execute_pos_command, AppState};
use apex_edge_contracts::{
    ContractVersion, PosCommand, PosRequestEnvelope, ReturnLineItemPayload, StartReturnPayload,
};
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use uuid::Uuid;
async fn fixture(inclusive: bool) -> (AppState, Uuid, Uuid) {
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let order = Uuid::new_v4();
    let line = Uuid::new_v4();
    sqlx::query("INSERT INTO orders (id,cart_id,store_id,register_id,total_cents,subtotal_cents,discount_cents,tax_cents,created_at,finalized_at) VALUES (?,?,?, ?,3190,3000,100,290,'2026-01-01','2026-01-01')").bind(order.to_string()).bind(Uuid::new_v4().to_string()).bind(Uuid::nil().to_string()).bind(Uuid::nil().to_string()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO order_lines (id,order_id,item_id,sku,name,quantity,unit_price_cents,line_total_cents,discount_cents,tax_cents,tax_inclusive,created_at) VALUES (?,?,?,'SKU','Item',3,1000,3000,100,290,?,'2026-01-01')").bind(line.to_string()).bind(order.to_string()).bind(Uuid::new_v4().to_string()).bind(inclusive).execute(&pool).await.unwrap();
    (AppState::new(pool, Uuid::nil()), order, line)
}
async fn send(
    state: &AppState,
    cmd: PosCommand,
) -> apex_edge_contracts::PosResponseEnvelope<serde_json::Value> {
    execute_pos_command(
        state,
        PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            payload: cmd,
        },
    )
    .await
}
async fn begin(state: &AppState, order: Uuid) -> Uuid {
    let r = send(
        state,
        PosCommand::StartReturn(StartReturnPayload {
            return_id: None,
            original_order_id: Some(order),
            reason_code: None,
            approval_id: None,
            shift_id: None,
        }),
    )
    .await;
    assert!(r.success, "{:?}", r.errors);
    r.payload.unwrap()["id"].as_str().unwrap().parse().unwrap()
}
fn line(ret: Uuid, original: Uuid, qty: u32) -> PosCommand {
    PosCommand::ReturnLineItem(ReturnLineItemPayload {
        return_id: ret,
        sku: "SKU".into(),
        name: None,
        quantity: qty,
        unit_price_cents: 999999,
        tax_cents: 999999,
        tax_inclusive: false,
        original_line_id: Some(original),
    })
}
#[tokio::test]
async fn receipted_returns_use_paid_values_and_preserve_rounding_across_partial_returns() {
    for inclusive in [false, true] {
        let (state, order, original) = fixture(inclusive).await;
        let mut gross = 0;
        let mut tax = 0;
        for _ in 0..3 {
            let ret = begin(&state, order).await;
            let r = send(&state, line(ret, original, 1)).await;
            assert!(r.success, "{:?}", r.errors);
            let p = r.payload.unwrap();
            gross += p["total_cents"].as_u64().unwrap();
            tax += p["tax_cents"].as_u64().unwrap();
        }
        assert_eq!(gross, if inclusive { 2900 } else { 3190 });
        assert_eq!(tax, 290);
    }
}
#[tokio::test]
async fn return_quantity_is_reserved_across_concurrent_return_documents() {
    let (state, order, original) = fixture(false).await;
    let a = begin(&state, order).await;
    let b = begin(&state, order).await;
    let (x, y) = tokio::join!(
        send(&state, line(a, original, 3)),
        send(&state, line(b, original, 3))
    );
    assert_ne!(
        x.success, y.success,
        "only original quantity may be returned"
    );
}
#[tokio::test]
async fn unrelated_original_line_is_rejected() {
    let (state, order, _) = fixture(false).await;
    let ret = begin(&state, order).await;
    assert!(!send(&state, line(ret, Uuid::new_v4(), 1)).await.success);
}
#[tokio::test]
async fn zero_quantity_is_rejected() {
    let (state, order, original) = fixture(false).await;
    let ret = begin(&state, order).await;
    assert!(!send(&state, line(ret, original, 0)).await.success);
}

#[tokio::test]
async fn voiding_an_unpaid_partial_return_releases_quantity_without_creating_rounding_credit() {
    let (state, order, original) = fixture(true).await;
    let first = begin(&state, order).await;
    assert!(send(&state, line(first, original, 1)).await.success);
    let second = begin(&state, order).await;
    assert!(send(&state, line(second, original, 1)).await.success);
    assert!(
        send(
            &state,
            PosCommand::VoidReturn(apex_edge_contracts::VoidReturnPayload {
                return_id: first,
                reason: None
            })
        )
        .await
        .success
    );
    let third = begin(&state, order).await;
    assert!(send(&state, line(third, original, 2)).await.success);
    let total:i64=sqlx::query_scalar("SELECT SUM(l.line_total_cents) FROM return_lines l JOIN returns r ON r.id=l.return_id WHERE r.state!='voided'").fetch_one(&state.pool).await.unwrap();
    assert_eq!(total, 2900);
}

#[tokio::test]
async fn a_refunded_return_cannot_be_voided_to_release_its_purchase_entitlement() {
    let (state, order, original) = fixture(false).await;
    let ret = begin(&state, order).await;
    assert!(send(&state, line(ret, original, 3)).await.success);
    let refund = send(
        &state,
        PosCommand::RefundTender(apex_edge_contracts::RefundTenderPayload {
            return_id: ret,
            tender_type: "cash".into(),
            amount_cents: 3190,
            external_reference: None,
        }),
    )
    .await;
    assert!(refund.success, "{:?}", refund.errors);
    assert!(
        !send(
            &state,
            PosCommand::VoidReturn(apex_edge_contracts::VoidReturnPayload {
                return_id: ret,
                reason: None
            })
        )
        .await
        .success
    );
}

#[tokio::test]
async fn legacy_voided_refunds_still_consume_original_entitlement() {
    let (state, order, original) = fixture(false).await;
    let ret = begin(&state, order).await;
    assert!(send(&state, line(ret, original, 3)).await.success);
    sqlx::query("UPDATE returns SET state='voided',refunded_cents=3190 WHERE id=?")
        .bind(ret.to_string())
        .execute(&state.pool)
        .await
        .unwrap();
    let next = begin(&state, order).await;
    assert!(!send(&state, line(next, original, 3)).await.success);
}
#[tokio::test]
async fn legacy_overvalued_lines_require_reconciliation_instead_of_negative_refund_lines() {
    let (state, order, original) = fixture(false).await;
    let ret = begin(&state, order).await;
    assert!(send(&state, line(ret, original, 1)).await.success);
    sqlx::query("UPDATE return_lines SET line_total_cents=999999 WHERE return_id=?")
        .bind(ret.to_string())
        .execute(&state.pool)
        .await
        .unwrap();
    let next = begin(&state, order).await;
    assert!(!send(&state, line(next, original, 2)).await.success);
}
