use apex_edge_adapters_payment::*;
use apex_edge_api::{pos_handler::execute_pos_command, AppState, PaymentSettings};
use apex_edge_contracts::{
    ContractVersion, PaymentProviderReceipt, PosCommand, PosRequestEnvelope, RefundTenderPayload,
};
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Default)]
struct Terminal {
    refunds: Mutex<HashSet<Uuid>>,
}
#[async_trait]
impl PaymentProvider for Terminal {
    fn provider_code(&self) -> &'static str {
        "test_terminal"
    }
    async fn authorize(
        &self,
        _: AuthorizeRequest,
    ) -> Result<PaymentAuthorization, PaymentProviderError> {
        Err(PaymentProviderError::InvalidAmount)
    }
    async fn capture(&self, _: CaptureRequest) -> Result<PaymentCapture, PaymentProviderError> {
        Err(PaymentProviderError::InvalidAmount)
    }
    async fn void(&self, _: VoidRequest) -> Result<(), PaymentProviderError> {
        Err(PaymentProviderError::InvalidAmount)
    }
    async fn refund(&self, r: RefundRequest) -> Result<PaymentRefund, PaymentProviderError> {
        self.refunds.lock().unwrap().insert(r.idempotency_key);
        Ok(PaymentRefund {
            provider: self.provider_code().into(),
            provider_refund_id: format!("refund_{}", r.idempotency_key),
            refunded_cents: r.amount_cents,
            receipt: PaymentProviderReceipt {
                provider: self.provider_code().into(),
                provider_payment_id: r.provider_payment_id,
                entry_method: None,
                last4: None,
                aid: None,
                authorization_code: None,
            },
        })
    }
}
async fn fixture() -> (AppState, Arc<Terminal>, PosRequestEnvelope<PosCommand>) {
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let order = Uuid::new_v4();
    let ret = Uuid::new_v4();
    sqlx::query("INSERT INTO orders (id,cart_id,store_id,register_id,total_cents,subtotal_cents,discount_cents,tax_cents,created_at,finalized_at) VALUES (?,?,?, ?,1000,1000,0,0,'2026-01-01','2026-01-01')").bind(order.to_string()).bind(Uuid::new_v4().to_string()).bind(Uuid::nil().to_string()).bind(Uuid::nil().to_string()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO order_payments (id,order_id,tender_id,tender_type,amount_cents,provider,provider_payment_id,created_at) VALUES (?,?,?,'test_terminal',1000,'test_terminal','pi_test','2026-01-01')").bind(Uuid::new_v4().to_string()).bind(order.to_string()).bind(Uuid::new_v4().to_string()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO returns (id,store_id,register_id,original_order_id,state,total_cents,tax_cents,refunded_cents,created_at) VALUES (?,?,?,?,'items',1000,0,0,'2026-01-01')").bind(ret.to_string()).bind(Uuid::nil().to_string()).bind(Uuid::nil().to_string()).bind(order.to_string()).execute(&pool).await.unwrap();
    let terminal = Arc::new(Terminal::default());
    let state = AppState {
        payments: PaymentSettings::new("EUR").with_provider(terminal.clone()),
        ..AppState::new(pool, Uuid::nil())
    };
    let request = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: Uuid::nil(),
        register_id: Uuid::nil(),
        payload: PosCommand::RefundTender(RefundTenderPayload {
            return_id: ret,
            tender_type: "test_terminal".into(),
            amount_cents: 1000,
            external_reference: None,
        }),
    };
    (state, terminal, request)
}
#[tokio::test]
async fn retry_after_provider_success_and_storage_failure_does_not_pay_twice() {
    let (state, terminal, request) = fixture().await;
    sqlx::query("CREATE TRIGGER fail_refund BEFORE INSERT ON refunds BEGIN SELECT RAISE(FAIL, 'simulated disk failure'); END").execute(&state.pool).await.unwrap();
    assert!(!execute_pos_command(&state, request.clone()).await.success);
    sqlx::query("DROP TRIGGER fail_refund")
        .execute(&state.pool)
        .await
        .unwrap();
    let restarted = AppState {
        payments: state.payments.clone(),
        ..AppState::new(state.pool.clone(), Uuid::nil())
    };
    let retry = execute_pos_command(&restarted, request.clone()).await;
    assert!(retry.success, "{:?}", retry.errors);
    assert_eq!(
        terminal.refunds.lock().unwrap().len(),
        1,
        "one provider refund across restart/retry"
    );
    assert!(
        execute_pos_command(&restarted, request).await.success,
        "persisted refund replays without response cache"
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM refunds")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
#[tokio::test]
async fn concurrent_refunds_cannot_reserve_more_than_the_return_total() {
    let (state, terminal, request) = fixture().await;
    let mut other = request.clone();
    other.idempotency_key = Uuid::new_v4();
    let (a, b) = tokio::join!(
        execute_pos_command(&state, request),
        execute_pos_command(&state, other)
    );
    assert_ne!(a.success, b.success, "only one full refund may succeed");
    assert_eq!(terminal.refunds.lock().unwrap().len(), 1);
}
