//! Edge auth: device pairing, token exchange, session lifecycle, and middleware.

use apex_edge_contracts::{
    AuthCreatePairingCodeRequest, AuthCreatePairingCodeResponse, AuthDevicePairRequest,
    AuthDevicePairResponse, AuthSessionExchangeRequest, AuthSessionExchangeResponse,
    AuthSessionRefreshRequest, AuthSessionRevokeResponse,
};
use apex_edge_metrics::{
    AUTH_REQUESTS_TOTAL, AUTH_REQUEST_DURATION_SECONDS, AUTH_SESSIONS_TOTAL, DEVICE_PAIRINGS_TOTAL,
};
use apex_edge_storage::{
    consume_pairing_code, create_auth_session, create_device_pairing_code, create_trusted_device,
    get_auth_session, get_pairing_code_by_hash, get_trusted_device,
    increment_pairing_code_attempts, record as record_audit, revoke_auth_session,
    upsert_associate_identity,
};
use axum::{
    body::Body,
    extract::{Extension, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::pos::AppState;

#[derive(Debug, Clone)]
pub struct AuthSettings {
    pub enabled: bool,
    pub external_issuer: String,
    pub external_audience: String,
    pub external_hs256_secret: Option<String>,
    pub external_public_key_pem: Option<String>,
    pub session_signing_secret: String,
    pub access_ttl_seconds: i64,
    pub refresh_ttl_seconds: i64,
    pub pairing_code_ttl_seconds: i64,
    pub pairing_code_length: usize,
    pub pairing_max_attempts: i64,
}

impl Default for AuthSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            external_issuer: String::new(),
            external_audience: String::new(),
            external_hs256_secret: None,
            external_public_key_pem: None,
            session_signing_secret: "dev-hub-secret".into(),
            access_ttl_seconds: 300,
            refresh_ttl_seconds: 3600,
            pairing_code_ttl_seconds: 300,
            pairing_code_length: 6,
            pairing_max_attempts: 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthPrincipal {
    pub session_id: Uuid,
    pub associate_id: String,
    pub device_id: Uuid,
    pub store_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExternalClaims {
    sub: String,
    iss: String,
    aud: String,
    exp: usize,
    iat: usize,
    store_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionClaims {
    sub: String,
    sid: String,
    did: String,
    store_id: String,
    typ: String,
    exp: usize,
    iat: usize,
}

fn hash_secret(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn issue_tokens(
    settings: &AuthSettings,
    session_id: Uuid,
    associate_id: &str,
    device_id: Uuid,
    store_id: Uuid,
) -> Result<(String, String, DateTime<Utc>, DateTime<Utc>), StatusCode> {
    let now = Utc::now();
    let access_exp = now + Duration::seconds(settings.access_ttl_seconds);
    let refresh_exp = now + Duration::seconds(settings.refresh_ttl_seconds);
    let encoding = EncodingKey::from_secret(settings.session_signing_secret.as_bytes());
    let access = SessionClaims {
        sub: associate_id.into(),
        sid: session_id.to_string(),
        did: device_id.to_string(),
        store_id: store_id.to_string(),
        typ: "access".into(),
        exp: access_exp.timestamp() as usize,
        iat: now.timestamp() as usize,
    };
    let refresh = SessionClaims {
        sub: associate_id.into(),
        sid: session_id.to_string(),
        did: device_id.to_string(),
        store_id: store_id.to_string(),
        typ: "refresh".into(),
        exp: refresh_exp.timestamp() as usize,
        iat: now.timestamp() as usize,
    };
    let access_token = encode(&Header::new(Algorithm::HS256), &access, &encoding)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let refresh_token = encode(&Header::new(Algorithm::HS256), &refresh, &encoding)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((access_token, refresh_token, access_exp, refresh_exp))
}

fn verify_external_token(
    settings: &AuthSettings,
    token: &str,
) -> Result<ExternalClaims, StatusCode> {
    let header = decode_header(token).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let alg = header.alg;
    let mut validation = Validation::new(match alg {
        Algorithm::HS256 => Algorithm::HS256,
        Algorithm::RS256 => Algorithm::RS256,
        _ => return Err(StatusCode::UNAUTHORIZED),
    });
    validation.set_issuer(&[settings.external_issuer.as_str()]);
    validation.set_audience(&[settings.external_audience.as_str()]);

    let claims = if alg == Algorithm::HS256 {
        let secret = settings
            .external_hs256_secret
            .as_ref()
            .ok_or(StatusCode::UNAUTHORIZED)?;
        decode::<ExternalClaims>(
            token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &validation,
        )
        .map_err(|_| StatusCode::UNAUTHORIZED)?
        .claims
    } else {
        let pem = settings
            .external_public_key_pem
            .as_ref()
            .ok_or(StatusCode::UNAUTHORIZED)?;
        decode::<ExternalClaims>(
            token,
            &DecodingKey::from_rsa_pem(pem.as_bytes()).map_err(|_| StatusCode::UNAUTHORIZED)?,
            &validation,
        )
        .map_err(|_| StatusCode::UNAUTHORIZED)?
        .claims
    };
    Ok(claims)
}

fn bearer_token(req: &Request<Body>) -> Option<String> {
    let raw = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ").map(|s| s.to_string())
}

fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        "/health"
            | "/ready"
            | "/metrics"
            | "/auth/pairing-codes"
            | "/auth/devices/pair"
            | "/auth/sessions/exchange"
            | "/auth/sessions/refresh"
    )
}

fn record_auth_metrics(operation: &'static str, outcome: &'static str, start: DateTime<Utc>) {
    metrics::counter!(AUTH_REQUESTS_TOTAL, 1u64, "operation" => operation, "outcome" => outcome);
    let elapsed = (Utc::now() - start).num_milliseconds() as f64 / 1000.0;
    metrics::histogram!(AUTH_REQUEST_DURATION_SECONDS, elapsed, "operation" => operation);
}

pub async fn auth_middleware(
    State(app): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    if !app.auth.enabled || is_public_path(req.uri().path()) {
        return next.run(req).await;
    }
    let token = match bearer_token(&req) {
        Some(t) => t,
        None => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_aud = false;
    validation.validate_exp = true;
    let decoded = match decode::<SessionClaims>(
        &token,
        &DecodingKey::from_secret(app.auth.session_signing_secret.as_bytes()),
        &validation,
    ) {
        Ok(v) => v.claims,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if decoded.typ != "access" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let session_id = match Uuid::parse_str(&decoded.sid) {
        Ok(v) => v,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let session = match get_auth_session(&app.pool, session_id).await {
        Ok(Some(v)) => v,
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if session.revoked_at.is_some() || session.access_exp < Utc::now() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let device = match get_trusted_device(&app.pool, session.device_id).await {
        Ok(Some(v)) => v,
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    };
    if device.revoked_at.is_some() || device.status != "active" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let claims_store = Uuid::parse_str(&decoded.store_id).unwrap_or_default();
    if claims_store != app.store_id || session.store_id != app.store_id {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    req.extensions_mut().insert(AuthPrincipal {
        session_id,
        associate_id: session.associate_id,
        device_id: session.device_id,
        store_id: session.store_id,
    });
    next.run(req).await
}

pub async fn create_pairing_code(
    State(app): State<AppState>,
    Json(req): Json<AuthCreatePairingCodeRequest>,
) -> Result<Json<AuthCreatePairingCodeResponse>, StatusCode> {
    let start = Utc::now();
    if !app.auth.enabled {
        record_auth_metrics("pairing_codes_create", "disabled", start);
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let code: String = {
        let mut rng = rand::thread_rng();
        (0..app.auth.pairing_code_length)
            .map(|_| char::from(b'0' + rng.gen_range(0..10) as u8))
            .collect()
    };
    let expires_at = Utc::now() + Duration::seconds(app.auth.pairing_code_ttl_seconds);
    let pairing_code_id = create_device_pairing_code(
        &app.pool,
        req.store_id,
        &hash_secret(&code),
        &req.created_by,
        expires_at,
        app.auth.pairing_max_attempts,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = record_audit(
        &app.pool,
        "auth_pairing_code_created",
        Some(pairing_code_id),
        &serde_json::json!({"store_id": req.store_id, "created_by": req.created_by}).to_string(),
    )
    .await;
    record_auth_metrics("pairing_codes_create", "ok", start);
    Ok(Json(AuthCreatePairingCodeResponse {
        pairing_code_id,
        code,
        expires_at,
    }))
}

pub async fn pair_device(
    State(app): State<AppState>,
    Json(req): Json<AuthDevicePairRequest>,
) -> Result<Json<AuthDevicePairResponse>, StatusCode> {
    let start = Utc::now();
    if !app.auth.enabled {
        record_auth_metrics("devices_pair", "disabled", start);
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let row = get_pairing_code_by_hash(&app.pool, &hash_secret(&req.pairing_code))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some(pairing) = row else {
        metrics::counter!(DEVICE_PAIRINGS_TOTAL, 1u64, "outcome" => "invalid_code");
        record_auth_metrics("devices_pair", "invalid_code", start);
        return Err(StatusCode::BAD_REQUEST);
    };
    if pairing.store_id != req.store_id {
        metrics::counter!(DEVICE_PAIRINGS_TOTAL, 1u64, "outcome" => "store_mismatch");
        record_auth_metrics("devices_pair", "store_mismatch", start);
        return Err(StatusCode::BAD_REQUEST);
    }
    if pairing.consumed_at.is_some()
        || pairing.expires_at < Utc::now()
        || pairing.attempts >= pairing.max_attempts
    {
        let _ = increment_pairing_code_attempts(&app.pool, pairing.id).await;
        metrics::counter!(DEVICE_PAIRINGS_TOTAL, 1u64, "outcome" => "expired_or_consumed");
        record_auth_metrics("devices_pair", "expired_or_consumed", start);
        return Err(StatusCode::BAD_REQUEST);
    }
    let device_id = Uuid::new_v4();
    let device_secret = format!("dev-{}-{}", Uuid::new_v4(), Uuid::new_v4());
    create_trusted_device(
        &app.pool,
        device_id,
        req.store_id,
        &req.device_name,
        req.platform.as_deref(),
        &hash_secret(&device_secret),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    consume_pairing_code(&app.pool, pairing.id, device_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = record_audit(
        &app.pool,
        "auth_device_paired",
        Some(device_id),
        &serde_json::json!({"store_id": req.store_id, "device_name": req.device_name, "platform": req.platform}).to_string(),
    )
    .await;
    metrics::counter!(DEVICE_PAIRINGS_TOTAL, 1u64, "outcome" => "ok");
    record_auth_metrics("devices_pair", "ok", start);
    Ok(Json(AuthDevicePairResponse {
        device_id,
        device_secret,
    }))
}

pub async fn exchange_session(
    State(app): State<AppState>,
    Json(req): Json<AuthSessionExchangeRequest>,
) -> Result<Json<AuthSessionExchangeResponse>, StatusCode> {
    let start = Utc::now();
    if !app.auth.enabled {
        record_auth_metrics("sessions_exchange", "disabled", start);
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let external = verify_external_token(&app.auth, &req.external_token)?;
    let store_id = Uuid::parse_str(&external.store_id).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let device = get_trusted_device(&app.pool, req.device_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if device.store_id != store_id
        || device.secret_hash != hash_secret(&req.device_secret)
        || device.status != "active"
        || device.revoked_at.is_some()
    {
        metrics::counter!(AUTH_SESSIONS_TOTAL, 1u64, "outcome" => "untrusted_device");
        record_auth_metrics("sessions_exchange", "untrusted_device", start);
        return Err(StatusCode::UNAUTHORIZED);
    }
    let session_id = Uuid::new_v4();
    let now = Utc::now();
    let access_exp = now + Duration::seconds(app.auth.access_ttl_seconds);
    let refresh_exp = now + Duration::seconds(app.auth.refresh_ttl_seconds);
    create_auth_session(
        &app.pool,
        session_id,
        &external.sub,
        store_id,
        req.device_id,
        access_exp,
        refresh_exp,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = upsert_associate_identity(
        &app.pool,
        &external.sub,
        store_id,
        external.name.as_deref(),
        external.email.as_deref(),
        &serde_json::to_string(&external).unwrap_or_else(|_| "{}".into()),
    )
    .await;
    let (access_token, refresh_token, expires_at, refresh_expires_at) = issue_tokens(
        &app.auth,
        session_id,
        &external.sub,
        req.device_id,
        store_id,
    )?;
    let _ = record_audit(
        &app.pool,
        "auth_session_issued",
        Some(session_id),
        &serde_json::json!({"associate_id": external.sub, "device_id": req.device_id, "store_id": store_id}).to_string(),
    )
    .await;
    metrics::counter!(AUTH_SESSIONS_TOTAL, 1u64, "outcome" => "issued");
    record_auth_metrics("sessions_exchange", "ok", start);
    Ok(Json(AuthSessionExchangeResponse {
        access_token,
        refresh_token,
        expires_at,
        refresh_expires_at,
    }))
}

pub async fn refresh_session(
    State(app): State<AppState>,
    Json(req): Json<AuthSessionRefreshRequest>,
) -> Result<Json<AuthSessionExchangeResponse>, StatusCode> {
    let start = Utc::now();
    if !app.auth.enabled {
        record_auth_metrics("sessions_refresh", "disabled", start);
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_aud = false;
    let claims = decode::<SessionClaims>(
        &req.refresh_token,
        &DecodingKey::from_secret(app.auth.session_signing_secret.as_bytes()),
        &validation,
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?
    .claims;
    if claims.typ != "refresh" {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let prior_session = Uuid::parse_str(&claims.sid).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let session = get_auth_session(&app.pool, prior_session)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if session.revoked_at.is_some() || session.refresh_exp < Utc::now() {
        metrics::counter!(AUTH_SESSIONS_TOTAL, 1u64, "outcome" => "refresh_denied");
        record_auth_metrics("sessions_refresh", "refresh_denied", start);
        return Err(StatusCode::UNAUTHORIZED);
    }
    revoke_auth_session(&app.pool, prior_session)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let session_id = Uuid::new_v4();
    let now = Utc::now();
    let access_exp = now + Duration::seconds(app.auth.access_ttl_seconds);
    let refresh_exp = now + Duration::seconds(app.auth.refresh_ttl_seconds);
    create_auth_session(
        &app.pool,
        session_id,
        &session.associate_id,
        session.store_id,
        session.device_id,
        access_exp,
        refresh_exp,
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let (access_token, refresh_token, expires_at, refresh_expires_at) = issue_tokens(
        &app.auth,
        session_id,
        &session.associate_id,
        session.device_id,
        session.store_id,
    )?;
    metrics::counter!(AUTH_SESSIONS_TOTAL, 1u64, "outcome" => "refreshed");
    record_auth_metrics("sessions_refresh", "ok", start);
    Ok(Json(AuthSessionExchangeResponse {
        access_token,
        refresh_token,
        expires_at,
        refresh_expires_at,
    }))
}

pub async fn revoke_session(
    State(app): State<AppState>,
    Extension(principal): Extension<AuthPrincipal>,
) -> Result<Json<AuthSessionRevokeResponse>, StatusCode> {
    let start = Utc::now();
    if !app.auth.enabled {
        record_auth_metrics("sessions_revoke", "disabled", start);
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    revoke_auth_session(&app.pool, principal.session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = record_audit(
        &app.pool,
        "auth_session_revoked",
        Some(principal.session_id),
        &serde_json::json!({"associate_id": principal.associate_id, "device_id": principal.device_id}).to_string(),
    )
    .await;
    metrics::counter!(AUTH_SESSIONS_TOTAL, 1u64, "outcome" => "revoked");
    record_auth_metrics("sessions_revoke", "ok", start);
    Ok(Json(AuthSessionRevokeResponse { revoked: true }))
}
