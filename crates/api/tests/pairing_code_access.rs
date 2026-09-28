//! Who may mint a device pairing code.
//!
//! A pairing code is the gate that turns an unknown device into a trusted register, so it
//! must not be mintable by anything that can reach the hub. Allowed: a request from the
//! hub machine itself (loopback), or one carrying an API token with the `pairing` scope
//! (or `admin` / `*`). Everything else is refused.

use std::net::SocketAddr;

use apex_edge_api::{create_pairing_code, AppState, AuthSettings};
use apex_edge_contracts::AuthCreatePairingCodeRequest;
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::Json;
use chrono::Utc;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::Serialize;
use uuid::Uuid;

const SECRET: &str = "pairing-access-test-secret";

#[derive(Serialize)]
struct ApiTokenClaims {
    sub: String,
    name: String,
    scopes: Vec<String>,
    exp: usize,
}

async fn hub() -> AppState {
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    AppState {
        auth: AuthSettings {
            enabled: true,
            session_signing_secret: SECRET.into(),
            ..AuthSettings::default()
        },
        ..AppState::new(pool, Uuid::nil())
    }
}

async fn api_token(app: &AppState, scopes: &[&str]) -> HeaderMap {
    let id = Uuid::new_v4();
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    sqlx::query("INSERT INTO api_tokens (id, name, scopes_json, created_at) VALUES (?, ?, ?, ?)")
        .bind(id.to_string())
        .bind("ops")
        .bind(serde_json::to_string(&scopes).unwrap())
        .bind(Utc::now().to_rfc3339())
        .execute(&app.pool)
        .await
        .unwrap();
    let jwt = encode(
        &Header::default(),
        &ApiTokenClaims {
            sub: id.to_string(),
            name: "ops".into(),
            scopes,
            exp: (Utc::now().timestamp() + 3600) as usize,
        },
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {jwt}")).unwrap(),
    );
    headers
}

async fn mint(app: &AppState, peer: Option<&str>, headers: HeaderMap) -> Result<(), StatusCode> {
    create_pairing_code(
        State(app.clone()),
        peer.map(|p| ConnectInfo(p.parse::<SocketAddr>().unwrap())),
        headers,
        Json(AuthCreatePairingCodeRequest {
            store_id: Uuid::nil(),
            created_by: "test".into(),
        }),
    )
    .await
    .map(|_| ())
}

#[tokio::test]
async fn the_hub_machine_itself_may_mint_a_code_without_a_token() {
    let app = hub().await;
    assert!(mint(&app, Some("127.0.0.1:50000"), HeaderMap::new())
        .await
        .is_ok());
    assert!(mint(&app, Some("[::1]:50000"), HeaderMap::new())
        .await
        .is_ok());
}

#[tokio::test]
async fn another_machine_without_a_token_is_refused() {
    let app = hub().await;
    assert_eq!(
        mint(&app, Some("192.168.1.50:50000"), HeaderMap::new()).await,
        Err(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        mint(&app, None, HeaderMap::new()).await,
        Err(StatusCode::FORBIDDEN),
        "an unknown peer is not trusted as loopback"
    );
}

#[tokio::test]
async fn another_machine_with_a_pairing_or_admin_token_is_allowed() {
    let app = hub().await;
    for scopes in [&["pairing"][..], &["admin"][..], &["*"][..]] {
        let headers = api_token(&app, scopes).await;
        assert!(
            mint(&app, Some("192.168.1.50:50000"), headers)
                .await
                .is_ok(),
            "{scopes:?}"
        );
    }
}

#[tokio::test]
async fn a_token_without_the_pairing_scope_or_a_forged_one_is_refused() {
    let app = hub().await;
    let pos_only = api_token(&app, &["pos"]).await;
    assert_eq!(
        mint(&app, Some("192.168.1.50:50000"), pos_only).await,
        Err(StatusCode::FORBIDDEN)
    );

    let mut forged = HeaderMap::new();
    forged.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer not-a-real-token"),
    );
    assert_eq!(
        mint(&app, Some("192.168.1.50:50000"), forged).await,
        Err(StatusCode::FORBIDDEN)
    );
}
