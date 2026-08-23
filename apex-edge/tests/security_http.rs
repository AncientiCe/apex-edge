//! Security hardening tests: API token scope enforcement and rate limiting over real HTTP.
//!
//! Auth-without-a-token and session-JWT coverage lives in `auth_http.rs`; this file covers
//! the pieces added for v2.0.0 "Prove It" harden-security that aren't exercised there yet.

use apex_edge::{build_router, HubConfig};
use apex_edge_api::{AuthSettings, RateLimitSettings};
use apex_edge_contracts::{ContractVersion, CreateCartPayload, PosCommand, PosRequestEnvelope};
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use chrono::Utc;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::Serialize;
use sqlx::SqlitePool;
use uuid::Uuid;

const SESSION_SECRET: &str = "hub-secret-for-security-tests";

#[derive(Serialize)]
struct ApiTokenClaims {
    sub: String,
    name: String,
    scopes: Vec<String>,
    exp: usize,
}

/// Insert an API token row directly (mirrors `admin_api::create_api_token`'s own insert) and
/// mint a matching JWT, sidestepping the admin-scope-to-create-a-token bootstrap problem.
async fn issue_api_token(pool: &SqlitePool, scopes: &[&str]) -> String {
    let token_id = Uuid::new_v4();
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    sqlx::query("INSERT INTO api_tokens (id, name, scopes_json, created_at) VALUES (?, ?, ?, ?)")
        .bind(token_id.to_string())
        .bind("test-token")
        .bind(serde_json::to_string(&scopes).unwrap())
        .bind(Utc::now().to_rfc3339())
        .execute(pool)
        .await
        .expect("insert api token");
    let claims = ApiTokenClaims {
        sub: token_id.to_string(),
        name: "test-token".into(),
        scopes,
        exp: (Utc::now().timestamp() + 3600) as usize,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(SESSION_SECRET.as_bytes()),
    )
    .expect("encode api token")
}

async fn start_server(store_id: Uuid, register_id: Uuid, config: HubConfig) -> (u16, SqlitePool) {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let app = build_router(
        pool.clone(),
        HubConfig {
            store_id,
            register_id,
            ..config
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    (port, pool)
}

fn create_cart_envelope(store_id: Uuid, register_id: Uuid) -> PosRequestEnvelope<PosCommand> {
    PosRequestEnvelope {
        version: ContractVersion::V1_0_0,
        idempotency_key: Uuid::new_v4(),
        store_id,
        register_id,
        payload: PosCommand::CreateCart(CreateCartPayload { cart_id: None }),
    }
}

#[tokio::test]
async fn api_token_with_insufficient_scope_is_forbidden() {
    let store_id = Uuid::nil();
    let register_id = Uuid::nil();
    let (port, pool) = start_server(
        store_id,
        register_id,
        HubConfig {
            auth: AuthSettings {
                enabled: true,
                session_signing_secret: SESSION_SECRET.into(),
                ..AuthSettings::default()
            },
            ..HubConfig::default()
        },
    )
    .await;

    let token = issue_api_token(&pool, &["catalog"]).await;
    let res = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/pos/command"))
        .bearer_auth(&token)
        .json(&create_cart_envelope(store_id, register_id))
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_token_with_matching_scope_is_allowed() {
    let store_id = Uuid::nil();
    let register_id = Uuid::nil();
    let (port, pool) = start_server(
        store_id,
        register_id,
        HubConfig {
            auth: AuthSettings {
                enabled: true,
                session_signing_secret: SESSION_SECRET.into(),
                ..AuthSettings::default()
            },
            ..HubConfig::default()
        },
    )
    .await;

    let token = issue_api_token(&pool, &["pos"]).await;
    let res = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/pos/command"))
        .bearer_auth(&token)
        .json(&create_cart_envelope(store_id, register_id))
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body.get("success"), Some(&serde_json::Value::Bool(true)));
}

#[tokio::test]
async fn requests_beyond_the_rate_limit_receive_429() {
    let store_id = Uuid::nil();
    let register_id = Uuid::nil();
    let (port, _pool) = start_server(
        store_id,
        register_id,
        HubConfig {
            rate_limit: RateLimitSettings {
                auth_per_minute: 30,
                pos_per_minute: 1,
            },
            ..HubConfig::default()
        },
    )
    .await;

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/pos/command");
    let first = client
        .post(&url)
        .json(&create_cart_envelope(store_id, register_id))
        .send()
        .await
        .expect("first request");
    assert_eq!(first.status(), reqwest::StatusCode::OK);

    let second = client
        .post(&url)
        .json(&create_cart_envelope(store_id, register_id))
        .send()
        .await
        .expect("second request");
    assert_eq!(second.status(), reqwest::StatusCode::TOO_MANY_REQUESTS);
    assert!(second.headers().contains_key("retry-after"));
}
