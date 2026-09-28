//! Associate time clock through the POS command surface.
//!
//! Clock entries feed payroll, so the rules are strict: an entry belongs to a real
//! associate, an associate has at most one open entry, and clocking out closes exactly
//! the entry that was opened.

use apex_edge_api::{pos_handler::execute_pos_command, AppState};
use apex_edge_contracts::{
    ClockInPayload, ClockOutPayload, ContractVersion, PosCommand, PosRequestEnvelope,
    PosResponseEnvelope,
};
use apex_edge_storage::{create_sqlite_pool, run_migrations, set_audit_key, AuditKey};
use serde_json::Value;
use uuid::Uuid;

async fn setup() -> AppState {
    set_audit_key(AuditKey::new("test-hub", b"test-secret".to_vec()));
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    AppState::new(pool, Uuid::nil())
}

async fn send(state: &AppState, command: PosCommand) -> PosResponseEnvelope<Value> {
    execute_pos_command(
        state,
        PosRequestEnvelope {
            version: ContractVersion::V1_0_0,
            idempotency_key: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: Uuid::nil(),
            payload: command,
        },
    )
    .await
}

fn clock_in(associate: &str) -> PosCommand {
    PosCommand::ClockIn(ClockInPayload {
        associate_id: associate.into(),
    })
}

fn clock_out(associate: &str) -> PosCommand {
    PosCommand::ClockOut(ClockOutPayload {
        associate_id: associate.into(),
    })
}

fn error_code(resp: &PosResponseEnvelope<Value>) -> &str {
    resp.errors.first().map(|e| e.code.as_str()).unwrap_or("")
}

#[tokio::test]
async fn clock_in_then_out_closes_the_same_entry() {
    let state = setup().await;

    let opened = send(&state, clock_in("  emp-7  ")).await;
    assert!(opened.success, "{:?}", opened.errors);
    let opened = opened.payload.expect("entry");
    assert_eq!(opened["associate_id"], "emp-7", "associate id is trimmed");
    assert!(opened["clocked_out_at"].is_null());

    let closed = send(&state, clock_out("emp-7")).await;
    assert!(closed.success, "{:?}", closed.errors);
    let closed = closed.payload.expect("entry");
    assert_eq!(closed["id"], opened["id"]);
    assert_eq!(closed["clocked_in_at"], opened["clocked_in_at"]);
    assert!(!closed["clocked_out_at"].is_null());
}

#[tokio::test]
async fn clocking_out_without_an_open_entry_is_rejected() {
    let state = setup().await;

    let resp = send(&state, clock_out("emp-7")).await;
    assert!(!resp.success);
    assert_eq!(error_code(&resp), "CLOCK_ENTRY_NOT_FOUND");

    assert!(send(&state, clock_in("emp-7")).await.success);
    assert!(send(&state, clock_out("emp-7")).await.success);
    let again = send(&state, clock_out("emp-7")).await;
    assert_eq!(error_code(&again), "CLOCK_ENTRY_NOT_FOUND");
}

#[tokio::test]
async fn a_blank_associate_id_is_rejected() {
    let state = setup().await;

    for command in [clock_in("   "), clock_out("")] {
        let resp = send(&state, command).await;
        assert!(!resp.success);
        assert_eq!(error_code(&resp), "INVALID_ASSOCIATE_ID");
        assert_eq!(
            resp.errors[0].field.as_deref(),
            Some("associate_id"),
            "points the POS at the offending field"
        );
    }
}

#[tokio::test]
async fn clocking_in_twice_is_rejected_so_payroll_never_sees_overlapping_entries() {
    let state = setup().await;

    assert!(send(&state, clock_in("emp-7")).await.success);
    let second = send(&state, clock_in("emp-7")).await;
    assert!(!second.success);
    assert_eq!(error_code(&second), "ALREADY_CLOCKED_IN");

    // A different associate is unaffected, and after clocking out emp-7 can clock in again.
    assert!(send(&state, clock_in("emp-8")).await.success);
    assert!(send(&state, clock_out("emp-7")).await.success);
    assert!(send(&state, clock_in("emp-7")).await.success);
}
