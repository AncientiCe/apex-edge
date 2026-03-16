use apex_edge::build_router;
use apex_edge_contracts::{
    AuthCreatePairingCodeRequest, AuthDevicePairRequest, AuthSessionExchangeRequest,
    AuthSessionRefreshRequest,
};
use apex_edge_storage::{create_sqlite_pool, run_migrations};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::Serialize;
use uuid::Uuid;

#[derive(Serialize)]
struct ExternalClaims {
    sub: String,
    iss: String,
    aud: String,
    exp: usize,
    iat: usize,
    store_id: String,
}

fn external_token(iss: &str, aud: &str, store_id: Uuid, secret: &str) -> String {
    let now = chrono::Utc::now().timestamp() as usize;
    let claims = ExternalClaims {
        sub: "associate-1".into(),
        iss: iss.into(),
        aud: aud.into(),
        exp: now + 600,
        iat: now,
        store_id: store_id.to_string(),
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("token")
}

async fn start_auth_server() -> (u16, Uuid) {
    let store_id = Uuid::nil();
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let app = build_router(
        pool,
        store_id,
        None,
        vec![],
        apex_edge_api::AuthSettings {
            enabled: true,
            external_issuer: "https://issuer.example".into(),
            external_audience: "mpos".into(),
            external_hs256_secret: Some("ext-secret".into()),
            external_public_key_pem: None,
            session_signing_secret: "hub-secret".into(),
            access_ttl_seconds: 300,
            refresh_ttl_seconds: 3600,
            pairing_code_ttl_seconds: 120,
            pairing_code_length: 6,
            pairing_max_attempts: 3,
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    (port, store_id)
}

#[tokio::test]
async fn protected_routes_require_auth_when_enabled() {
    let (port, _) = start_auth_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/catalog/products"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pairing_code_is_one_time_and_device_can_exchange_session() {
    let (port, store_id) = start_auth_server().await;
    let client = reqwest::Client::new();

    let create = client
        .post(format!("http://127.0.0.1:{port}/auth/pairing-codes"))
        .json(&AuthCreatePairingCodeRequest {
            store_id,
            created_by: "admin".into(),
        })
        .send()
        .await
        .expect("create pairing");
    assert_eq!(create.status(), reqwest::StatusCode::OK);
    let pairing = create
        .json::<apex_edge_contracts::AuthCreatePairingCodeResponse>()
        .await
        .expect("pairing response");

    let pair = client
        .post(format!("http://127.0.0.1:{port}/auth/devices/pair"))
        .json(&AuthDevicePairRequest {
            pairing_code: pairing.code.clone(),
            store_id,
            device_name: "iPad POS".into(),
            platform: Some("ios".into()),
        })
        .send()
        .await
        .expect("pair device");
    assert_eq!(pair.status(), reqwest::StatusCode::OK);
    let device = pair
        .json::<apex_edge_contracts::AuthDevicePairResponse>()
        .await
        .expect("device");

    let pair_again = client
        .post(format!("http://127.0.0.1:{port}/auth/devices/pair"))
        .json(&AuthDevicePairRequest {
            pairing_code: pairing.code,
            store_id,
            device_name: "iPad POS".into(),
            platform: Some("ios".into()),
        })
        .send()
        .await
        .expect("pair again");
    assert_eq!(pair_again.status(), reqwest::StatusCode::BAD_REQUEST);

    let ext = external_token("https://issuer.example", "mpos", store_id, "ext-secret");
    let exchange = client
        .post(format!("http://127.0.0.1:{port}/auth/sessions/exchange"))
        .json(&AuthSessionExchangeRequest {
            external_token: ext,
            device_id: device.device_id,
            device_secret: device.device_secret.clone(),
        })
        .send()
        .await
        .expect("exchange");
    assert_eq!(exchange.status(), reqwest::StatusCode::OK);
    let sess = exchange
        .json::<apex_edge_contracts::AuthSessionExchangeResponse>()
        .await
        .expect("session");

    let ok = client
        .get(format!("http://127.0.0.1:{port}/catalog/products"))
        .bearer_auth(&sess.access_token)
        .send()
        .await
        .expect("authorized");
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn exchange_rejects_invalid_claims_and_untrusted_device() {
    let (port, store_id) = start_auth_server().await;
    let client = reqwest::Client::new();
    let bad_iss = external_token("https://wrong.example", "mpos", store_id, "ext-secret");
    let bad = client
        .post(format!("http://127.0.0.1:{port}/auth/sessions/exchange"))
        .json(&AuthSessionExchangeRequest {
            external_token: bad_iss,
            device_id: Uuid::new_v4(),
            device_secret: "secret".into(),
        })
        .send()
        .await
        .expect("exchange bad");
    assert_eq!(bad.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn refresh_rotates_and_revoke_invalidates_session() {
    let (port, store_id) = start_auth_server().await;
    let client = reqwest::Client::new();
    let pairing = client
        .post(format!("http://127.0.0.1:{port}/auth/pairing-codes"))
        .json(&AuthCreatePairingCodeRequest {
            store_id,
            created_by: "admin".into(),
        })
        .send()
        .await
        .expect("pairing")
        .json::<apex_edge_contracts::AuthCreatePairingCodeResponse>()
        .await
        .expect("pairing body");
    let device = client
        .post(format!("http://127.0.0.1:{port}/auth/devices/pair"))
        .json(&AuthDevicePairRequest {
            pairing_code: pairing.code,
            store_id,
            device_name: "test-device".into(),
            platform: None,
        })
        .send()
        .await
        .expect("device pair")
        .json::<apex_edge_contracts::AuthDevicePairResponse>()
        .await
        .expect("device body");
    let ext = external_token("https://issuer.example", "mpos", store_id, "ext-secret");
    let session = client
        .post(format!("http://127.0.0.1:{port}/auth/sessions/exchange"))
        .json(&AuthSessionExchangeRequest {
            external_token: ext,
            device_id: device.device_id,
            device_secret: device.device_secret,
        })
        .send()
        .await
        .expect("exchange")
        .json::<apex_edge_contracts::AuthSessionExchangeResponse>()
        .await
        .expect("exchange body");

    let refreshed = client
        .post(format!("http://127.0.0.1:{port}/auth/sessions/refresh"))
        .json(&AuthSessionRefreshRequest {
            refresh_token: session.refresh_token,
        })
        .send()
        .await
        .expect("refresh");
    assert_eq!(refreshed.status(), reqwest::StatusCode::OK);
    let refreshed = refreshed
        .json::<apex_edge_contracts::AuthSessionExchangeResponse>()
        .await
        .expect("refresh body");

    let revoke = client
        .post(format!("http://127.0.0.1:{port}/auth/sessions/revoke"))
        .bearer_auth(&refreshed.access_token)
        .send()
        .await
        .expect("revoke");
    assert_eq!(revoke.status(), reqwest::StatusCode::OK);

    let denied = client
        .get(format!("http://127.0.0.1:{port}/catalog/products"))
        .bearer_auth(&refreshed.access_token)
        .send()
        .await
        .expect("denied");
    assert_eq!(denied.status(), reqwest::StatusCode::UNAUTHORIZED);
}
