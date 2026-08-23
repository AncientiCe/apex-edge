//! Payment intent ledger: the record that lets the hub answer, after a crash,
//! whether money was taken and whether the customer got a sale for it.

use apex_edge_storage::*;
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

async fn test_pool() -> sqlx::SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    pool
}

fn intent(store_id: Uuid, cart_id: Uuid, idempotency_key: Uuid) -> NewPaymentIntent {
    NewPaymentIntent {
        store_id,
        register_id: Uuid::new_v4(),
        cart_id,
        tender_id: Uuid::new_v4(),
        idempotency_key,
        provider: "simulated_terminal".into(),
        amount_cents: 2_500,
        tip_amount_cents: 0,
    }
}

#[tokio::test]
async fn an_intent_starts_authorized_and_can_be_read_back() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();

    let record = insert_payment_intent(&pool, intent(store_id, cart_id, Uuid::new_v4()))
        .await
        .expect("insert intent");

    assert_eq!(record.state, PaymentIntentState::Authorized);
    assert_eq!(record.amount_cents, 2_500);

    let loaded = get_payment_intent(&pool, record.id)
        .await
        .expect("load intent")
        .expect("intent exists");
    assert_eq!(loaded.id, record.id);
    assert_eq!(loaded.cart_id, cart_id);
}

#[tokio::test]
async fn reusing_an_idempotency_key_returns_the_existing_intent() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();
    let key = Uuid::new_v4();

    let mut first = intent(store_id, cart_id, key);
    first.tender_id = Uuid::nil();
    let mut second = intent(store_id, cart_id, key);
    second.tender_id = Uuid::nil();

    let a = insert_payment_intent(&pool, first)
        .await
        .expect("first insert");
    let b = insert_payment_intent(&pool, second)
        .await
        .expect("retry must not create a second charge");

    assert_eq!(a.id, b.id);
    let all = list_payment_intents_for_cart(&pool, store_id, cart_id)
        .await
        .expect("list");
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn capture_then_settle_walks_the_state_machine() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();
    let order_id = Uuid::new_v4();

    let record = insert_payment_intent(&pool, intent(store_id, cart_id, Uuid::new_v4()))
        .await
        .expect("insert");

    mark_payment_intent_captured(&pool, record.id, "sim_abc", 2_500)
        .await
        .expect("capture");
    let captured = get_payment_intent(&pool, record.id)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(captured.state, PaymentIntentState::Captured);
    assert_eq!(captured.provider_payment_id.as_deref(), Some("sim_abc"));
    assert_eq!(captured.approved_cents, 2_500);

    settle_payment_intents_for_cart(&pool, store_id, cart_id, order_id)
        .await
        .expect("settle");
    let settled = get_payment_intent(&pool, record.id)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(settled.state, PaymentIntentState::Settled);
    assert_eq!(settled.order_id, Some(order_id));
}

#[tokio::test]
async fn a_captured_intent_whose_sale_never_completed_is_owed_a_reversal() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();

    let record = insert_payment_intent(&pool, intent(store_id, cart_id, Uuid::new_v4()))
        .await
        .expect("insert");
    mark_payment_intent_captured(&pool, record.id, "sim_abc", 2_500)
        .await
        .expect("capture");

    // The finalize that should have followed never happened.
    flag_payment_intents_for_reversal(&pool, store_id, cart_id, "finalize failed")
        .await
        .expect("flag");

    let owed = fetch_payment_intents_awaiting_reversal(&pool, 10)
        .await
        .expect("fetch owed");
    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].id, record.id);
    assert_eq!(owed[0].state, PaymentIntentState::ReversalPending);
    assert_eq!(owed[0].provider_payment_id.as_deref(), Some("sim_abc"));
}

#[tokio::test]
async fn settled_intents_are_never_offered_for_reversal() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();

    let record = insert_payment_intent(&pool, intent(store_id, cart_id, Uuid::new_v4()))
        .await
        .expect("insert");
    mark_payment_intent_captured(&pool, record.id, "sim_abc", 2_500)
        .await
        .expect("capture");
    settle_payment_intents_for_cart(&pool, store_id, cart_id, Uuid::new_v4())
        .await
        .expect("settle");

    flag_payment_intents_for_reversal(&pool, store_id, cart_id, "late failure")
        .await
        .expect("flag");

    let owed = fetch_payment_intents_awaiting_reversal(&pool, 10)
        .await
        .expect("fetch owed");
    assert!(
        owed.is_empty(),
        "a paid, ledgered sale must never be reversed"
    );
}

#[tokio::test]
async fn a_completed_reversal_leaves_the_queue() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();

    let record = insert_payment_intent(&pool, intent(store_id, cart_id, Uuid::new_v4()))
        .await
        .expect("insert");
    mark_payment_intent_captured(&pool, record.id, "sim_abc", 2_500)
        .await
        .expect("capture");
    flag_payment_intents_for_reversal(&pool, store_id, cart_id, "finalize failed")
        .await
        .expect("flag");

    mark_payment_intent_reversed(&pool, record.id)
        .await
        .expect("reversed");

    assert!(fetch_payment_intents_awaiting_reversal(&pool, 10)
        .await
        .expect("fetch owed")
        .is_empty());
    let reversed = get_payment_intent(&pool, record.id)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(reversed.state, PaymentIntentState::Reversed);
}

#[tokio::test]
async fn a_failed_reversal_is_retried_with_its_attempt_counted() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();

    let record = insert_payment_intent(&pool, intent(store_id, cart_id, Uuid::new_v4()))
        .await
        .expect("insert");
    mark_payment_intent_captured(&pool, record.id, "sim_abc", 2_500)
        .await
        .expect("capture");
    flag_payment_intents_for_reversal(&pool, store_id, cart_id, "finalize failed")
        .await
        .expect("flag");

    record_payment_intent_reversal_failure(&pool, record.id, "provider unreachable")
        .await
        .expect("record failure");

    let owed = fetch_payment_intents_awaiting_reversal(&pool, 10)
        .await
        .expect("fetch owed");
    assert_eq!(
        owed.len(),
        1,
        "an unreversed capture must stay in the queue"
    );
    assert_eq!(owed[0].reversal_attempts, 1);
    assert_eq!(owed[0].last_error.as_deref(), Some("provider unreachable"));
}

#[tokio::test]
async fn a_capture_left_behind_by_a_crash_is_eventually_flagged() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();

    let record = insert_payment_intent(&pool, intent(store_id, cart_id, Uuid::new_v4()))
        .await
        .expect("insert");
    mark_payment_intent_captured(&pool, record.id, "sim_abc", 2_500)
        .await
        .expect("capture");

    // Nothing flagged it: the process died before any handler ran.
    assert_eq!(
        flag_stale_captured_payment_intents(&pool, 3_600)
            .await
            .expect("flag stale"),
        0,
        "a checkout in progress must not be reversed out from under the operator"
    );

    assert_eq!(
        flag_stale_captured_payment_intents(&pool, 0)
            .await
            .expect("flag stale"),
        1
    );
    let owed = fetch_payment_intents_awaiting_reversal(&pool, 10)
        .await
        .expect("fetch owed");
    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].id, record.id);
}

#[tokio::test]
async fn a_settled_capture_is_never_treated_as_orphaned() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();

    let record = insert_payment_intent(&pool, intent(store_id, cart_id, Uuid::new_v4()))
        .await
        .expect("insert");
    mark_payment_intent_captured(&pool, record.id, "sim_abc", 2_500)
        .await
        .expect("capture");
    settle_payment_intents_for_cart(&pool, store_id, cart_id, Uuid::new_v4())
        .await
        .expect("settle");

    assert_eq!(
        flag_stale_captured_payment_intents(&pool, 0)
            .await
            .expect("flag stale"),
        0
    );
}

#[tokio::test]
async fn a_decline_is_recorded_without_owing_a_reversal() {
    let pool = test_pool().await;
    let store_id = Uuid::new_v4();
    let cart_id = Uuid::new_v4();

    let record = insert_payment_intent(&pool, intent(store_id, cart_id, Uuid::new_v4()))
        .await
        .expect("insert");
    mark_payment_intent_declined(&pool, record.id, "card_declined")
        .await
        .expect("decline");

    let declined = get_payment_intent(&pool, record.id)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(declined.state, PaymentIntentState::Declined);
    assert_eq!(declined.failure_code.as_deref(), Some("card_declined"));

    assert!(fetch_payment_intents_awaiting_reversal(&pool, 10)
        .await
        .expect("fetch owed")
        .is_empty());
}
