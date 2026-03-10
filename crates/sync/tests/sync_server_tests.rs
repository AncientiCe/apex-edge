//! Sync tests against an in-process server: one endpoint per entity, config-driven fetch, progress %.

use apex_edge_contracts::ContractVersion;
use apex_edge_storage::{get_sync_checkpoint, run_migrations};
use apex_edge_sync::{fetch_all, ingest_batch, ConflictPolicy, SyncEntityConfig, SyncSourceConfig};
use axum::{routing::get, Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::Serialize;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::net::TcpListener;

/// Response shape for sync endpoints (must match SyncEndpointResponse in fetch.rs).
#[derive(Serialize)]
struct SyncResponse {
    items: Vec<String>,
    total: u64,
}

async fn sync_catalog() -> Json<SyncResponse> {
    Json(SyncResponse {
        items: vec![
            BASE64.encode(b"catalog-item-1"),
            BASE64.encode(b"catalog-item-2"),
        ],
        total: 2,
    })
}

async fn sync_price_book() -> Json<SyncResponse> {
    Json(SyncResponse {
        items: vec![BASE64.encode(b"price-entry-1")],
        total: 1,
    })
}

async fn sync_tax_rules() -> Json<SyncResponse> {
    Json(SyncResponse {
        items: vec![BASE64.encode(b"tax-rule-1"), BASE64.encode(b"tax-rule-2")],
        total: 2,
    })
}

async fn sync_promotions() -> Json<SyncResponse> {
    Json(SyncResponse {
        items: vec![BASE64.encode(b"promo-1")],
        total: 1,
    })
}

async fn sync_customers() -> Json<SyncResponse> {
    Json(SyncResponse {
        items: vec![BASE64.encode(b"customer-1")],
        total: 1,
    })
}

fn sync_router() -> Router {
    Router::new()
        .route("/sync/catalog", get(sync_catalog))
        .route("/sync/price_book", get(sync_price_book))
        .route("/sync/tax_rules", get(sync_tax_rules))
        .route("/sync/promotions", get(sync_promotions))
        .route("/sync/customers", get(sync_customers))
}

async fn start_sync_server() -> (u16, String) {
    let app = sync_router();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let base = format!("http://127.0.0.1:{}", port);
    tokio::spawn(async move {
        let result = axum::serve(listener, app).await;
        if let Err(e) = result {
            eprintln!("serve ended: {}", e);
        }
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    (port, base)
}

#[tokio::test]
async fn fetch_from_sync_server_each_entity_on_own_endpoint() {
    let (port, base_url) = start_sync_server().await;
    assert!(
        base_url.contains(port.to_string().as_str()),
        "base_url should contain port"
    );
    let client = reqwest::Client::new();
    let config = SyncSourceConfig {
        base_url: base_url.clone(),
        entities: vec![
            SyncEntityConfig {
                entity: "catalog".into(),
                path: "/sync/catalog".into(),
            },
            SyncEntityConfig {
                entity: "price_book".into(),
                path: "/sync/price_book".into(),
            },
            SyncEntityConfig {
                entity: "tax_rules".into(),
                path: "/sync/tax_rules".into(),
            },
            SyncEntityConfig {
                entity: "promotions".into(),
                path: "/sync/promotions".into(),
            },
            SyncEntityConfig {
                entity: "customers".into(),
                path: "/sync/customers".into(),
            },
        ],
    };

    let entities_called: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
    let current_by_entity = |entity: &str| {
        entities_called.borrow_mut().insert(entity.to_string());
        None::<i64>
    };
    let (results, summary) = fetch_all(&client, &config, current_by_entity)
        .await
        .expect("fetch_all");

    assert_eq!(
        entities_called.borrow().len(),
        5,
        "callback should see all 5 entities"
    );
    assert_eq!(results.len(), 5);
    let catalog = results.iter().find(|(e, _, _)| e == "catalog").unwrap();
    assert_eq!(catalog.1.len(), 2);
    assert_eq!(catalog.1[0], b"catalog-item-1");
    assert_eq!(catalog.1[1], b"catalog-item-2");
    assert_eq!(catalog.2, 2);

    let price_book = results.iter().find(|(e, _, _)| e == "price_book").unwrap();
    assert_eq!(price_book.1.len(), 1);
    assert_eq!(price_book.1[0], b"price-entry-1");
    assert_eq!(price_book.2, 1);

    assert_eq!(summary.entities.len(), 5);
    assert!(summary.overall_percent.is_some());
    let pct = summary.overall_percent.unwrap();
    assert!((0.0..=100.0).contains(&pct));
}

#[tokio::test]
async fn sync_data_progress_percent_and_ingest_with_any_progress() {
    let (port, base_url) = start_sync_server().await;
    assert!(base_url.contains(port.to_string().as_str()));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let config = SyncSourceConfig {
        base_url: base_url.clone(),
        entities: vec![
            SyncEntityConfig {
                entity: "catalog".into(),
                path: "/sync/catalog".into(),
            },
            SyncEntityConfig {
                entity: "price_book".into(),
                path: "/sync/price_book".into(),
            },
        ],
    };

    let client = reqwest::Client::new();
    let mut checkpoints = std::collections::HashMap::new();
    for ent in &config.entities {
        let seq = get_sync_checkpoint(&pool, &ent.entity)
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
        checkpoints.insert(ent.entity.clone(), seq);
    }
    let current_by_entity = |entity: &str| checkpoints.get(entity).copied();

    let (results, summary) = fetch_all(&client, &config, current_by_entity)
        .await
        .expect("fetch_all");

    assert!(summary.overall_percent.is_some());
    let pct_before = summary.overall_percent.unwrap();
    assert!((0.0..=100.0).contains(&pct_before));

    // Ingest only catalog (partial progress is valid)
    for (entity, payloads, total) in &results {
        assert_eq!(
            *total,
            payloads.len() as u64,
            "total should match payload count"
        );
        if entity == "catalog" && !payloads.is_empty() {
            ingest_batch(
                &pool,
                entity,
                ContractVersion::V1_0_0,
                payloads,
                ConflictPolicy::HqWins,
            )
            .await
            .expect("ingest catalog");
        }
    }

    let catalog_seq = get_sync_checkpoint(&pool, "catalog")
        .await
        .expect("get")
        .unwrap();
    assert_eq!(catalog_seq, 2);

    // Progress after partial ingest: catalog 2/2, price_book 0/1 (usage allowed at any progress)
    let summary2 = apex_edge_sync::SyncProgressSummary::from_entities(vec![
        apex_edge_sync::SyncEntityProgress {
            entity: "catalog".into(),
            current: 2,
            total: Some(2),
        },
        apex_edge_sync::SyncEntityProgress {
            entity: "price_book".into(),
            current: 0,
            total: Some(1),
        },
    ]);
    assert!(!summary2.is_complete());
    assert!(summary2.overall_percent.unwrap() < 100.0);
}

// --- NDJSON streaming tests (require fetch_entity_ndjson_stream and streamed ingest) ---

/// Helper: start a server that serves one entity as NDJSON (first line = {"total": N}, then N lines of base64 payload).
/// Serves valid CatalogItem JSON so apply_entity_batch can parse it.
async fn start_ndjson_sync_server() -> (u16, String) {
    use apex_edge_contracts::CatalogItem;
    use axum::body::Body;
    use axum::http::Response;
    use axum::routing::get;
    use uuid::Uuid;

    fn make_catalog_ndjson_body() -> String {
        let item1 = CatalogItem {
            id: Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(),
            sku: "NDJSON-001".into(),
            name: "NDJSON Item 1".into(),
            description: None,
            category_id: Uuid::nil(),
            tax_category_id: Uuid::nil(),
            modifiers: vec![],
            is_active: true,
            version: 1,
        };
        let item2 = CatalogItem {
            id: Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap(),
            sku: "NDJSON-002".into(),
            name: "NDJSON Item 2".into(),
            description: None,
            category_id: Uuid::nil(),
            tax_category_id: Uuid::nil(),
            modifiers: vec![],
            is_active: true,
            version: 1,
        };
        let b64_1 = BASE64.encode(serde_json::to_vec(&item1).unwrap());
        let b64_2 = BASE64.encode(serde_json::to_vec(&item2).unwrap());
        format!("{{\"total\":2}}\n\"{b64_1}\"\n\"{b64_2}\"")
    }

    async fn ndjson_catalog() -> Response<Body> {
        Response::builder()
            .header("content-type", "application/x-ndjson")
            .body(Body::from(make_catalog_ndjson_body()))
            .unwrap()
    }

    let app = Router::new().route("/sync/ndjson/catalog", get(ndjson_catalog));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let base = format!("http://127.0.0.1:{}", port);
    tokio::spawn(async move {
        let result = axum::serve(listener, app).await;
        if let Err(e) = result {
            eprintln!("serve ended: {}", e);
        }
    });
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    (port, base)
}

#[tokio::test]
async fn fetch_entity_ndjson_stream_yields_payloads_incrementally() {
    let (port, base_url) = start_ndjson_sync_server().await;
    assert!(base_url.contains(port.to_string().as_str()));
    let client = reqwest::Client::new();
    let url = format!("{}/sync/ndjson/catalog", base_url.trim_end_matches('/'));

    let mut collected: Vec<Vec<u8>> = Vec::new();
    let mut progress_updates: Vec<u64> = Vec::new();

    apex_edge_sync::fetch_entity_ndjson_stream(
        &client,
        &url,
        0,
        |payloads: &[Vec<u8>], total: u64| {
            collected.extend(payloads.iter().cloned());
            progress_updates.push(total);
        },
    )
    .await
    .expect("fetch_entity_ndjson_stream");

    assert_eq!(collected.len(), 2, "should collect two payloads");
    // Payloads are valid CatalogItem JSON bytes
    let item1: apex_edge_contracts::CatalogItem =
        serde_json::from_slice(&collected[0]).expect("first payload should be valid CatalogItem");
    let item2: apex_edge_contracts::CatalogItem =
        serde_json::from_slice(&collected[1]).expect("second payload should be valid CatalogItem");
    assert_eq!(item1.sku, "NDJSON-001");
    assert_eq!(item2.sku, "NDJSON-002");
    assert!(
        !progress_updates.is_empty(),
        "progress callback should be invoked at least once"
    );
    assert_eq!(
        *progress_updates.last().unwrap(),
        2,
        "final progress total should be 2"
    );
}

#[tokio::test]
async fn streamed_ingest_advances_checkpoint_per_batch() {
    let (port, base_url) = start_ndjson_sync_server().await;
    assert!(base_url.contains(port.to_string().as_str()));
    let client = reqwest::Client::new();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("pool");
    apex_edge_storage::run_migrations(&pool)
        .await
        .expect("migrations");

    let config = apex_edge_sync::SyncSourceConfig {
        base_url: base_url.clone(),
        entities: vec![apex_edge_sync::SyncEntityConfig {
            entity: "catalog".into(),
            path: "/sync/ndjson/catalog".into(),
        }],
    };

    apex_edge_sync::run_sync_ndjson(
        &client,
        &pool,
        &config,
        apex_edge_contracts::ContractVersion::V1_0_0,
        uuid::Uuid::nil(),
    )
    .await
    .expect("run_sync_ndjson");

    let seq = apex_edge_storage::get_sync_checkpoint(&pool, "catalog")
        .await
        .expect("get checkpoint")
        .unwrap();
    assert_eq!(
        seq, 2,
        "checkpoint should advance to 2 after ingesting 2 items"
    );
}
