//! End-to-end orchestrator journey test:
//! boot app, run POS flows (create cart, search/add product, customer, promo, payment, finalize),
//! verify document and HQ payload.

use apex_edge::build_router;
use apex_edge_contracts::HqOrderSubmissionEnvelope;
use apex_edge_contracts::{AddLineItemPayload, AddPaymentPayload, SetTenderingPayload};
use apex_edge_contracts::{
    ContractVersion, CreateCartPayload, FinalizeOrderPayload, PosCommand, PosRequestEnvelope,
    SetCustomerPayload,
};
use apex_edge_contracts::{PromoAction, PromoCondition, Promotion, PromotionType};
use apex_edge_storage::{
    enqueue_document, fetch_pending_outbox, insert_catalog_item, insert_customer,
    insert_price_book_entry, insert_promotion, insert_tax_rule, mark_generated, run_migrations,
};
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use sqlx::sqlite::SqlitePoolOptions;
use tokio::net::TcpListener;
use uuid::Uuid;

const STORE_ID: Uuid = Uuid::nil();
const REGISTER_ID: Uuid = Uuid::nil();

async fn start_app() -> (u16, sqlx::SqlitePool) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let app = build_router(pool.clone(), STORE_ID, None, vec![]);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = reqwest::Client::new();
    for _ in 0..30 {
        if client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .map(|r| r.status() == StatusCode::OK)
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    (port, pool)
}

/// Seed catalog, price book, tax, promotion (20% off when 2 items), and customer for full flow test.
async fn seed_full_flow_data(pool: &sqlx::SqlitePool) {
    let category_id = Uuid::nil();
    let tax_category_id = Uuid::nil();

    let product1_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let product2_id = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
    insert_catalog_item(
        pool,
        product1_id,
        STORE_ID,
        "SKU1",
        "Product One",
        category_id,
        tax_category_id,
    )
    .await
    .expect("insert product1");
    insert_catalog_item(
        pool,
        product2_id,
        STORE_ID,
        "SKU2",
        "Product Two",
        category_id,
        tax_category_id,
    )
    .await
    .expect("insert product2");

    insert_price_book_entry(pool, STORE_ID, product1_id, None, 1000, "USD")
        .await
        .expect("price product1");
    insert_price_book_entry(pool, STORE_ID, product2_id, None, 1000, "USD")
        .await
        .expect("price product2");

    let tax_rule_id = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
    insert_tax_rule(
        pool,
        tax_rule_id,
        STORE_ID,
        tax_category_id,
        0,
        "No tax",
        false,
    )
    .await
    .expect("insert tax rule");

    let promo_id = Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap();
    let promo = Promotion {
        id: promo_id,
        code: None,
        name: "20% off 2 products".into(),
        promo_type: PromotionType::PercentageOff { percent_bps: 2000 },
        priority: 10,
        valid_from: Utc::now() - Duration::days(1),
        valid_until: Some(Utc::now() + Duration::days(1)),
        conditions: vec![PromoCondition::MinBasketAmount { amount_cents: 1 }],
        actions: vec![PromoAction::ApplyToBasket],
        version: 1,
    };
    let promo_json = serde_json::to_string(&promo).expect("serialize promo");
    insert_promotion(pool, promo_id, STORE_ID, &promo_json)
        .await
        .expect("insert promo");

    let customer_id = Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap();
    insert_customer(pool, customer_id, STORE_ID, "CUST01", "Test Customer", None)
        .await
        .expect("insert customer");
}

async fn start_app_with_seed() -> (u16, sqlx::SqlitePool) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");
    seed_full_flow_data(&pool).await;

    let app = build_router(pool.clone(), STORE_ID, None, vec![]);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let client = reqwest::Client::new();
    for _ in 0..30 {
        if client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .map(|r| r.status() == StatusCode::OK)
            .unwrap_or(false)
        {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    (port, pool)
}

#[tokio::test]
async fn orchestrator_full_journey() {
    let (port, pool) = start_app().await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .expect("health request");
    assert_eq!(health.status(), StatusCode::OK);

    let ready = client
        .get(format!("{base}/ready"))
        .send()
        .await
        .expect("ready request");
    assert_eq!(ready.status(), StatusCode::OK);

    let invalid_idempotency = Uuid::new_v4();
    let invalid_req = PosRequestEnvelope {
        version: ContractVersion::new(9, 0, 0),
        idempotency_key: invalid_idempotency,
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    };
    let invalid_res = client
        .post(format!("{base}/pos/command"))
        .json(&invalid_req)
        .send()
        .await
        .expect("invalid pos request");
    assert_eq!(invalid_res.status(), StatusCode::OK);
    let invalid_json: serde_json::Value = invalid_res.json().await.expect("invalid json");
    assert_eq!(
        invalid_json.get("success"),
        Some(&serde_json::Value::Bool(false))
    );
    assert_eq!(
        invalid_json
            .get("errors")
            .and_then(|e| e.as_array())
            .and_then(|a| a.first())
            .and_then(|e| e.get("code"))
            .and_then(|v| v.as_str()),
        Some("UNSUPPORTED_VERSION")
    );

    let valid_idempotency = Uuid::new_v4();
    let valid_req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: valid_idempotency,
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    };
    let valid_res = client
        .post(format!("{base}/pos/command"))
        .json(&valid_req)
        .send()
        .await
        .expect("valid pos request");
    assert_eq!(valid_res.status(), StatusCode::OK);
    let valid_json: serde_json::Value = valid_res.json().await.expect("valid json");
    assert_eq!(
        valid_json.get("success"),
        Some(&serde_json::Value::Bool(true))
    );

    let order_id = Uuid::new_v4();
    let doc_id = Uuid::new_v4();
    enqueue_document(
        &pool,
        doc_id,
        "receipt",
        Some(order_id),
        None,
        Uuid::new_v4(),
        r#"{"total_cents":1234}"#,
    )
    .await
    .expect("enqueue doc");
    mark_generated(&pool, doc_id, "text/plain", "receipt content")
        .await
        .expect("mark generated");

    let list_res = client
        .get(format!("{base}/orders/{order_id}/documents"))
        .send()
        .await
        .expect("list docs request");
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_json: serde_json::Value = list_res.json().await.expect("list docs json");
    let first = list_json
        .as_array()
        .and_then(|a| a.first())
        .expect("at least one document");
    let doc_id_str = doc_id.to_string();
    assert_eq!(
        first.get("id").and_then(|v| v.as_str()),
        Some(doc_id_str.as_str())
    );
    assert_eq!(
        first.get("status").and_then(|v| v.as_str()),
        Some("generated")
    );

    let doc_res = client
        .get(format!("{base}/documents/{doc_id}"))
        .send()
        .await
        .expect("get doc request");
    assert_eq!(doc_res.status(), StatusCode::OK);
    let doc_json: serde_json::Value = doc_res.json().await.expect("doc json");
    assert_eq!(
        doc_json.get("content").and_then(|v| v.as_str()),
        Some("receipt content")
    );
}

#[tokio::test]
async fn full_order_flow_journey() {
    let (port, pool) = start_app_with_seed().await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // 1) Create cart
    let cart_id = Uuid::new_v4();
    let create_req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: PosCommand::CreateCart(CreateCartPayload {
            cart_id: Some(cart_id),
        }),
    };
    let create_res = client
        .post(format!("{base}/pos/command"))
        .json(&create_req)
        .send()
        .await
        .expect("create cart");
    assert_eq!(create_res.status(), StatusCode::OK);
    let create_json: serde_json::Value = create_res.json().await.expect("create json");
    assert_eq!(
        create_json.get("success"),
        Some(&serde_json::Value::Bool(true))
    );
    let payload = create_json.get("payload").expect("payload");
    assert_eq!(
        payload.get("cart_id").and_then(|v| v.as_str()),
        Some(cart_id.to_string().as_str())
    );

    // 2) Search product
    let search_res = client
        .get(format!("{base}/catalog/products?sku=SKU1"))
        .send()
        .await
        .expect("search product");
    assert_eq!(search_res.status(), StatusCode::OK);
    let products: Vec<serde_json::Value> = search_res.json().await.expect("products json");
    assert!(!products.is_empty());
    let product1_id = products[0]
        .get("id")
        .and_then(|v| v.as_str())
        .expect("product id");
    let product1_uuid = Uuid::parse_str(product1_id).expect("parse product id");

    // 3) Add product
    let add1_req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: PosCommand::AddLineItem(AddLineItemPayload {
            cart_id,
            item_id: product1_uuid,
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }),
    };
    let add1_res = client
        .post(format!("{base}/pos/command"))
        .json(&add1_req)
        .send()
        .await
        .expect("add line item");
    assert_eq!(add1_res.status(), StatusCode::OK);
    let add1_json: serde_json::Value = add1_res.json().await.expect("add1 json");
    assert_eq!(
        add1_json.get("success"),
        Some(&serde_json::Value::Bool(true))
    );

    // 4) Search customer
    let cust_res = client
        .get(format!("{base}/customers?code=CUST01"))
        .send()
        .await
        .expect("search customer");
    assert_eq!(cust_res.status(), StatusCode::OK);
    let customers: Vec<serde_json::Value> = cust_res.json().await.expect("customers json");
    assert!(!customers.is_empty());
    let customer_id_str = customers[0]
        .get("id")
        .and_then(|v| v.as_str())
        .expect("customer id");
    let customer_id = Uuid::parse_str(customer_id_str).expect("parse customer id");

    // 5) Add customer
    let set_cust_req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: PosCommand::SetCustomer(SetCustomerPayload {
            cart_id,
            customer_id,
        }),
    };
    let set_cust_res = client
        .post(format!("{base}/pos/command"))
        .json(&set_cust_req)
        .send()
        .await
        .expect("set customer");
    assert_eq!(set_cust_res.status(), StatusCode::OK);
    let set_cust_json: serde_json::Value = set_cust_res.json().await.expect("set customer json");
    assert_eq!(
        set_cust_json.get("success"),
        Some(&serde_json::Value::Bool(true))
    );

    // 6) Search second product and add
    let search2_res = client
        .get(format!("{base}/catalog/products?sku=SKU2"))
        .send()
        .await
        .expect("search product 2");
    assert_eq!(search2_res.status(), StatusCode::OK);
    let products2: Vec<serde_json::Value> = search2_res.json().await.expect("products2 json");
    assert!(!products2.is_empty());
    let product2_id_str = products2[0]
        .get("id")
        .and_then(|v| v.as_str())
        .expect("product2 id");
    let product2_uuid = Uuid::parse_str(product2_id_str).expect("parse product2 id");

    let add2_req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: PosCommand::AddLineItem(AddLineItemPayload {
            cart_id,
            item_id: product2_uuid,
            modifier_option_ids: vec![],
            quantity: 1,
            notes: None,
            unit_price_override_cents: None,
        }),
    };
    let add2_res = client
        .post(format!("{base}/pos/command"))
        .json(&add2_req)
        .send()
        .await
        .expect("add second product");
    assert_eq!(add2_res.status(), StatusCode::OK);
    let add2_json: serde_json::Value = add2_res.json().await.expect("add2 json");
    assert_eq!(
        add2_json.get("success"),
        Some(&serde_json::Value::Bool(true))
    );

    // 7) Assert automatic 20% promotion: subtotal 2000, discount 400, total 1600
    let cart_payload = add2_json.get("payload").expect("cart payload");
    let subtotal = cart_payload
        .get("subtotal_cents")
        .and_then(|v| v.as_u64())
        .expect("subtotal_cents");
    let discount = cart_payload
        .get("discount_cents")
        .and_then(|v| v.as_u64())
        .expect("discount_cents");
    let total = cart_payload
        .get("total_cents")
        .and_then(|v| v.as_u64())
        .expect("total_cents");
    assert_eq!(subtotal, 2000, "subtotal should be 1000+1000");
    assert_eq!(discount, 400, "20% off 2000 = 400");
    assert_eq!(total, 1600, "total after discount 1600");

    // 8) Set tendering and receive payment
    let tendering_req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: PosCommand::SetTendering(SetTenderingPayload { cart_id }),
    };
    let tendering_res = client
        .post(format!("{base}/pos/command"))
        .json(&tendering_req)
        .send()
        .await
        .expect("set tendering");
    assert_eq!(tendering_res.status(), StatusCode::OK);

    let tender_id = Uuid::new_v4();
    let payment_req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: PosCommand::AddPayment(AddPaymentPayload {
            cart_id,
            tender_id,
            amount_cents: 1600,
            external_reference: None,
        }),
    };
    let payment_res = client
        .post(format!("{base}/pos/command"))
        .json(&payment_req)
        .send()
        .await
        .expect("add payment");
    assert_eq!(payment_res.status(), StatusCode::OK);
    let payment_json: serde_json::Value = payment_res.json().await.expect("payment json");
    assert_eq!(
        payment_json.get("success"),
        Some(&serde_json::Value::Bool(true))
    );

    // 9) Place order (finalize)
    let finalize_req = PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id: STORE_ID,
        register_id: REGISTER_ID,
        payload: PosCommand::FinalizeOrder(FinalizeOrderPayload { cart_id }),
    };
    let finalize_res = client
        .post(format!("{base}/pos/command"))
        .json(&finalize_req)
        .send()
        .await
        .expect("finalize order");
    assert_eq!(finalize_res.status(), StatusCode::OK);
    let finalize_json: serde_json::Value = finalize_res.json().await.expect("finalize json");
    assert_eq!(
        finalize_json.get("success"),
        Some(&serde_json::Value::Bool(true))
    );
    let finalize_payload = finalize_json.get("payload").expect("finalize payload");
    let order_id_str = finalize_payload
        .get("order_id")
        .and_then(|v| v.as_str())
        .expect("order_id");
    let order_id = Uuid::parse_str(order_id_str).expect("parse order_id");
    assert_eq!(
        finalize_payload.get("total_cents").and_then(|v| v.as_u64()),
        Some(1600)
    );
    let print_job_ids = finalize_payload
        .get("print_job_ids")
        .and_then(|v| v.as_array())
        .expect("print_job_ids");
    assert!(!print_job_ids.is_empty());

    // 10) Generate document (done by finalize) - list and get document, assert content
    let list_docs_res = client
        .get(format!("{base}/orders/{order_id}/documents"))
        .send()
        .await
        .expect("list order documents");
    assert_eq!(list_docs_res.status(), StatusCode::OK);
    let docs_list: Vec<serde_json::Value> = list_docs_res.json().await.expect("docs list");
    assert!(!docs_list.is_empty(), "at least one document");
    let doc_id_str = docs_list[0]
        .get("id")
        .and_then(|v| v.as_str())
        .expect("doc id");
    let doc_id = Uuid::parse_str(doc_id_str).expect("parse doc id");

    let get_doc_res = client
        .get(format!("{base}/documents/{doc_id}"))
        .send()
        .await
        .expect("get document");
    assert_eq!(get_doc_res.status(), StatusCode::OK);
    let doc_body: serde_json::Value = get_doc_res.json().await.expect("doc body");
    let content = doc_body
        .get("content")
        .and_then(|v| v.as_str())
        .expect("content");
    assert!(
        content.contains(order_id_str) || content.contains(&order_id.to_string()),
        "document content should contain order_id"
    );
    assert!(
        content.contains("1600"),
        "document content should contain total 1600"
    );

    // 11) Assert HQ capture payload (outbox) contains correct full payload
    let pending = fetch_pending_outbox(&pool, 10).await.expect("fetch outbox");
    assert!(!pending.is_empty(), "outbox should have one submission");
    let outbox_row = &pending[0];
    let envelope: HqOrderSubmissionEnvelope =
        serde_json::from_str(&outbox_row.payload).expect("parse HQ envelope");
    assert_eq!(envelope.order.order_id, order_id);
    assert_eq!(envelope.order.cart_id, cart_id);
    assert_eq!(envelope.order.total_cents, 1600);
    assert_eq!(envelope.order.subtotal_cents, 2000);
    assert_eq!(envelope.order.discount_cents, 400);
    assert_eq!(envelope.order.tax_cents, 0);
    assert_eq!(envelope.order.lines.len(), 2);
    assert_eq!(envelope.order.payments.len(), 1);
    assert_eq!(envelope.order.payments[0].amount_cents, 1600);
    assert!(!envelope.checksum.is_empty());
    assert_eq!(envelope.store_id, STORE_ID);
    assert_eq!(envelope.register_id, REGISTER_ID);
}
