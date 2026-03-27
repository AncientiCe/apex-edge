use std::sync::Arc;

use apex_edge_contracts::{
    build_submission_envelope, HqAppliedCoupon, HqOrderLine, HqOrderPayload, HqPayment,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use chrono::Utc;
use fake_hq::{build_app, storage::Storage, AppState};
use tower::util::ServiceExt;
use uuid::Uuid;

fn sample_payload(order_id: Uuid) -> HqOrderPayload {
    HqOrderPayload {
        order_id,
        cart_id: Uuid::new_v4(),
        created_at: Utc::now(),
        lines: vec![HqOrderLine {
            line_id: Uuid::new_v4(),
            item_id: Uuid::new_v4(),
            sku: "SKU-1".to_string(),
            name: "Demo Item".to_string(),
            quantity: 2,
            unit_price_cents: 500,
            line_total_cents: 1000,
            discount_cents: 0,
            tax_cents: 0,
            modifier_option_ids: vec![],
            notes: None,
        }],
        subtotal_cents: 1000,
        discount_cents: 0,
        tax_cents: 0,
        total_cents: 1000,
        payments: vec![HqPayment {
            tender_id: Uuid::new_v4(),
            amount_cents: 1000,
            external_reference: Some("cash".to_string()),
        }],
        applied_promo_ids: vec![],
        applied_coupons: vec![HqAppliedCoupon {
            coupon_id: Uuid::new_v4(),
            code: "SAVE20".to_string(),
            discount_cents: 0,
        }],
        metadata: None,
    }
}

fn sample_envelope(submission_id: Uuid, order_id: Uuid, sequence_number: u64) -> serde_json::Value {
    let envelope = build_submission_envelope(
        submission_id,
        Uuid::from_u128(1),
        Uuid::from_u128(2),
        sequence_number,
        sample_payload(order_id),
    );
    serde_json::to_value(envelope).expect("serialize envelope")
}

fn test_db_path() -> String {
    let file_name = format!("fake-hq-test-{}.db", Uuid::new_v4());
    std::env::temp_dir().join(file_name).display().to_string()
}

#[tokio::test]
async fn storage_insert_list_get_and_duplicate_are_idempotent() {
    let path = test_db_path();
    let storage = Storage::open(&path).expect("open storage");
    storage.init_schema().expect("init schema");

    let submission_id = Uuid::new_v4();
    let order_id = Uuid::new_v4();
    let envelope: apex_edge_contracts::HqOrderSubmissionEnvelope =
        serde_json::from_value(sample_envelope(submission_id, order_id, 1)).expect("envelope");

    let first = storage.insert_order(&envelope).expect("first insert");
    assert!(first.inserted);
    let second = storage.insert_order(&envelope).expect("duplicate insert");
    assert!(!second.inserted);

    let page = storage.list_orders(1, 20).expect("list orders");
    assert_eq!(page.total, 1);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].submission_id, submission_id);

    let detail = storage
        .get_order(submission_id)
        .expect("get order")
        .expect("order exists");
    assert_eq!(detail.order_id, order_id);
}

#[tokio::test]
async fn post_orders_and_read_paginated_listing_and_detail() {
    let path = test_db_path();
    let storage = Arc::new(Storage::open(&path).expect("open storage"));
    storage.init_schema().expect("init schema");
    let app_state = Arc::new(AppState {
        storage,
        metrics_handle: None,
    });

    let app = build_app(app_state);

    for i in 0..25 {
        let body = sample_envelope(Uuid::new_v4(), Uuid::new_v4(), i + 1);
        let req = Request::builder()
            .method("POST")
            .uri("/api/orders")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&body).expect("serialize request"),
            ))
            .expect("request");
        let res = app.clone().oneshot(req).await.expect("post /api/orders");
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), 1024 * 1024)
            .await
            .expect("read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("parse body");
        assert_eq!(body["accepted"], true);
    }

    let list_req = Request::builder()
        .method("GET")
        .uri("/api/orders?page=2&per_page=10")
        .body(Body::empty())
        .expect("request");
    let list_res = app.clone().oneshot(list_req).await.expect("get listing");
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_bytes = to_bytes(list_res.into_body(), 1024 * 1024)
        .await
        .expect("read list body");
    let list_body: serde_json::Value = serde_json::from_slice(&list_bytes).expect("list json");
    assert_eq!(list_body["page"], 2);
    assert_eq!(list_body["per_page"], 10);
    assert_eq!(list_body["total"], 25);
    assert_eq!(
        list_body["items"].as_array().expect("items is array").len(),
        10
    );

    let first_submission_id = list_body["items"].as_array().expect("items")[0]["submission_id"]
        .as_str()
        .expect("submission id");
    let detail_req = Request::builder()
        .method("GET")
        .uri(format!("/api/orders/{first_submission_id}"))
        .body(Body::empty())
        .expect("detail request");
    let detail_res = app.clone().oneshot(detail_req).await.expect("get detail");
    assert_eq!(detail_res.status(), StatusCode::OK);
    let detail_bytes = to_bytes(detail_res.into_body(), 1024 * 1024)
        .await
        .expect("read detail");
    let detail_json: serde_json::Value =
        serde_json::from_slice(&detail_bytes).expect("detail json");
    assert_eq!(detail_json["submission_id"], first_submission_id);
    assert!(detail_json.get("payload_json").is_some());
}

#[tokio::test]
async fn post_duplicate_submission_id_is_idempotent() {
    let path = test_db_path();
    let storage = Arc::new(Storage::open(&path).expect("open storage"));
    storage.init_schema().expect("init schema");
    let app_state = Arc::new(AppState {
        storage,
        metrics_handle: None,
    });
    let app = build_app(app_state);

    let submission_id = Uuid::new_v4();
    let payload = sample_envelope(submission_id, Uuid::new_v4(), 1);
    for _ in 0..2 {
        let req = Request::builder()
            .method("POST")
            .uri("/api/orders")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&payload).expect("serialize payload"),
            ))
            .expect("post req");
        let res = app.clone().oneshot(req).await.expect("post");
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), 1024 * 1024)
            .await
            .expect("read body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["accepted"], true);
        assert_eq!(body["submission_id"], submission_id.to_string());
    }

    let list_req = Request::builder()
        .method("GET")
        .uri("/api/orders")
        .body(Body::empty())
        .expect("list req");
    let list_res = app.clone().oneshot(list_req).await.expect("list");
    let bytes = to_bytes(list_res.into_body(), 1024 * 1024)
        .await
        .expect("read body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("list json");
    assert_eq!(body["total"], 1);
}
