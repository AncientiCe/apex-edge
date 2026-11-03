use apex_edge_api::{deny_approval_handler, grant_approval_handler, AppState, AuthPrincipal};
use apex_edge_storage::{
    create_sqlite_pool, fetch_approval, request_approval, run_migrations, upsert_associate_identity,
};
use axum::{routing::post, Extension, Router};
use uuid::Uuid;

async fn decision(
    roles: &[&str],
    actor: Option<&str>,
    requester: &str,
    action: &str,
) -> (u16, Option<String>) {
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let state = AppState::new(pool.clone(), Uuid::nil());
    let approval = request_approval(
        &pool,
        Uuid::nil(),
        None,
        "blind_return",
        Some(requester),
        "{}",
        300,
    )
    .await
    .unwrap();
    let mut app = Router::new()
        .route("/approvals/:id/grant", post(grant_approval_handler))
        .route("/approvals/:id/deny", post(deny_approval_handler))
        .with_state(state);
    if let Some(actor) = actor {
        upsert_associate_identity(
            &pool,
            actor,
            Uuid::nil(),
            None,
            None,
            &serde_json::json!({"roles":roles}).to_string(),
        )
        .await
        .unwrap();
        app = app.layer(Extension(AuthPrincipal {
            session_id: Uuid::new_v4(),
            associate_id: actor.into(),
            device_id: Uuid::new_v4(),
            store_id: Uuid::nil(),
            register_id: None,
        }));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let body=serde_json::json!({"approval_id":approval.id,"approver_id":"forged-manager","reason":"test"}).to_string();
    let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
    socket.write_all(format!("POST /approvals/{}/{action} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",approval.id,body.len(),body).as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    socket.read_to_end(&mut bytes).await.unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let status = text
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    server.abort();
    (
        status,
        fetch_approval(&pool, approval.id)
            .await
            .unwrap()
            .unwrap()
            .approver_id,
    )
}
#[tokio::test]
async fn ordinary_associate_cannot_grant_or_deny() {
    for action in ["grant", "deny"] {
        assert_eq!(decision(&[], Some("cashier"), "other", action).await.0, 403)
    }
}
#[tokio::test]
async fn approval_decisions_require_an_authenticated_principal() {
    assert_eq!(decision(&[], None, "cashier", "grant").await.0, 401)
}
#[tokio::test]
async fn manager_decision_records_authenticated_identity() {
    let (status, id) = decision(&["manager"], Some("real-manager"), "cashier", "grant").await;
    assert_eq!(status, 200);
    assert_eq!(id.as_deref(), Some("real-manager"));
}
#[tokio::test]
async fn even_a_manager_cannot_approve_their_own_request() {
    assert_eq!(
        decision(&["manager"], Some("manager"), "manager", "grant")
            .await
            .0,
        403
    )
}

#[tokio::test]
async fn cashier_cannot_mint_privileged_api_tokens_to_bypass_approval_checks() {
    use apex_edge_api::auth_middleware;
    use apex_edge_storage::{create_auth_session, create_trusted_device};
    use chrono::{Duration, Utc};
    use jsonwebtoken::{encode, EncodingKey, Header};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut state = AppState::new(pool.clone(), Uuid::nil());
    state.auth.enabled = true;
    let sid = Uuid::new_v4();
    let did = Uuid::new_v4();
    create_trusted_device(&pool, did, Uuid::nil(), Uuid::nil(), "test", None, "secret")
        .await
        .unwrap();
    create_auth_session(
        &pool,
        sid,
        "cashier",
        Uuid::nil(),
        did,
        Utc::now() + Duration::hours(1),
        Utc::now() + Duration::hours(2),
    )
    .await
    .unwrap();
    let claims = serde_json::json!({"sub":"cashier","sid":sid,"did":did,"store_id":Uuid::nil(),"typ":"access","iat":Utc::now().timestamp(),"exp":(Utc::now()+Duration::hours(1)).timestamp()});
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.auth.session_signing_secret.as_bytes()),
    )
    .unwrap();
    let app = Router::new()
        .route("/admin/api-tokens", post(|| async { "must not run" }))
        .route_layer(axum::middleware::from_fn_with_state(state, auth_middleware));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
    socket.write_all(format!("POST /admin/api-tokens HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    socket.read_to_end(&mut bytes).await.unwrap();
    server.abort();
    assert!(String::from_utf8(bytes)
        .unwrap()
        .starts_with("HTTP/1.1 403"));
}
