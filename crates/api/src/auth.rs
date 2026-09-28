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

/// Parse a boolean env flag. Unset uses `default` (production auth is on unless
/// `APEX_EDGE_AUTH_ENABLED` is explicitly `0`/`false`/`no`).
pub fn parse_enabled_flag(raw: Option<&str>, default: bool) -> bool {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => default,
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
    }
}

/// Where the session signing secret came from; reported at boot and as a metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningSecretSource {
    /// `APEX_EDGE_AUTH_SESSION_SIGNING_SECRET`.
    Env,
    /// Read from the key file written on an earlier boot.
    FileLoaded,
    /// No secret existed; a random one was generated and written to the key file.
    FileGenerated,
    /// Random for this process only: auth is disabled, or the database is in memory, so
    /// there is nothing a persisted secret would protect across restarts.
    Ephemeral,
}

impl SigningSecretSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::FileLoaded => "file_loaded",
            Self::FileGenerated => "file_generated",
            Self::Ephemeral => "ephemeral",
        }
    }
}

/// Where the session key file lives for a given `APEX_EDGE_DB`: next to a file database,
/// or `None` for an in-memory one. Accepts plain paths and `sqlite:` URLs.
pub fn session_key_path_for_db(db: &str) -> Option<std::path::PathBuf> {
    let (path, query) = match db.strip_prefix("sqlite:") {
        Some(url) => {
            let (path, query) = url.split_once('?').unwrap_or((url, ""));
            let path = path.strip_prefix("//").unwrap_or(path);
            (path.strip_prefix("file:").unwrap_or(path), query)
        }
        None => (db, ""),
    };
    let in_memory =
        path.is_empty() || path == ":memory:" || query.split('&').any(|kv| kv == "mode=memory");
    if in_memory {
        return None;
    }
    Some(std::path::Path::new(path).with_file_name("apex_edge_session.key"))
}

/// Hex characters in a generated secret (32 random bytes). Shorter key files are refused.
const GENERATED_SECRET_HEX_LEN: usize = 64;

fn random_secret() -> String {
    hex::encode(rand::thread_rng().gen::<[u8; 32]>())
}

/// Resolves the secret that signs device sessions and admin API tokens.
///
/// An explicit non-blank `env_value` wins. Otherwise the secret is read from `key_path`,
/// or generated and written there (owner-only on Unix) on first boot, so it survives
/// restarts without ever falling back to a value anyone could guess. A key file that
/// exists but is too short is an error rather than a silently weak key.
pub fn resolve_session_signing_secret(
    env_value: Option<String>,
    key_path: &std::path::Path,
) -> std::io::Result<(String, SigningSecretSource)> {
    if let Some(secret) = env_value.filter(|v| !v.trim().is_empty()) {
        return Ok((secret, SigningSecretSource::Env));
    }
    match std::fs::read_to_string(key_path) {
        Ok(contents) => {
            let secret = contents.trim().to_string();
            if secret.len() < GENERATED_SECRET_HEX_LEN {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "session signing key file {} is too short; delete it to regenerate",
                        key_path.display()
                    ),
                ));
            }
            Ok((secret, SigningSecretSource::FileLoaded))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = key_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let secret = random_secret();
            write_owner_only(key_path, &secret)?;
            Ok((secret, SigningSecretSource::FileGenerated))
        }
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn write_owner_only(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn write_owner_only(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

impl AuthSettings {
    /// The signing secret for an explicit value and an optional key-file location. With no
    /// location there is nothing durable to keep a key in, so the secret is ephemeral.
    pub fn secret_for(
        env_value: Option<String>,
        key_path: Option<&std::path::Path>,
    ) -> std::io::Result<(String, SigningSecretSource)> {
        match key_path {
            Some(path) => resolve_session_signing_secret(env_value, path).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "session signing key file {}: {e} (set APEX_EDGE_AUTH_SESSION_KEY_PATH \
                         to a writable location or APEX_EDGE_AUTH_SESSION_SIGNING_SECRET)",
                        path.display()
                    ),
                )
            }),
            None => match env_value.filter(|v| !v.trim().is_empty()) {
                Some(secret) => Ok((secret, SigningSecretSource::Env)),
                None => Ok((random_secret(), SigningSecretSource::Ephemeral)),
            },
        }
    }

    /// Production defaults: auth is on unless the env flag turns it off.
    ///
    /// `default_key_path` is where the session signing secret is kept when
    /// `APEX_EDGE_AUTH_SESSION_SIGNING_SECRET` is unset (`None` for an in-memory database);
    /// `APEX_EDGE_AUTH_SESSION_KEY_PATH` overrides it. With auth disabled no key file is
    /// touched, since no token is ever checked.
    pub fn from_env(
        default_key_path: Option<&std::path::Path>,
    ) -> std::io::Result<(Self, SigningSecretSource)> {
        let enabled = parse_enabled_flag(
            std::env::var("APEX_EDGE_AUTH_ENABLED").ok().as_deref(),
            true,
        );
        let key_path = std::env::var("APEX_EDGE_AUTH_SESSION_KEY_PATH")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| default_key_path.map(std::path::Path::to_path_buf));
        let (session_signing_secret, source) = Self::secret_for(
            std::env::var("APEX_EDGE_AUTH_SESSION_SIGNING_SECRET").ok(),
            key_path.as_deref().filter(|_| enabled),
        )?;
        let external_public_key_pem = std::env::var("APEX_EDGE_AUTH_EXTERNAL_PUBLIC_KEY_PEM_PATH")
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok());
        let settings = Self {
            enabled,
            external_issuer: std::env::var("APEX_EDGE_AUTH_EXTERNAL_ISSUER").unwrap_or_default(),
            external_audience: std::env::var("APEX_EDGE_AUTH_EXTERNAL_AUDIENCE")
                .unwrap_or_default(),
            external_hs256_secret: std::env::var("APEX_EDGE_AUTH_EXTERNAL_HS256_SECRET").ok(),
            external_public_key_pem,
            session_signing_secret,
            access_ttl_seconds: std::env::var("APEX_EDGE_AUTH_ACCESS_TTL_SECONDS")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(300),
            refresh_ttl_seconds: std::env::var("APEX_EDGE_AUTH_REFRESH_TTL_SECONDS")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(3600),
            pairing_code_ttl_seconds: std::env::var("APEX_EDGE_AUTH_PAIRING_CODE_TTL_SECONDS")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(300),
            pairing_code_length: std::env::var("APEX_EDGE_AUTH_PAIRING_CODE_LENGTH")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(6),
            pairing_max_attempts: std::env::var("APEX_EDGE_AUTH_PAIRING_MAX_ATTEMPTS")
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(3),
        };
        Ok((settings, source))
    }
}

impl Default for AuthSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            external_issuer: String::new(),
            external_audience: String::new(),
            external_hs256_secret: None,
            external_public_key_pem: None,
            // Random per instance: a default must never be a secret anyone else knows.
            session_signing_secret: random_secret(),
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
    pub register_id: Option<Uuid>,
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

pub fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        "/health"
            | "/ready"
            | "/metrics"
            | "/auth/pairing-codes"
            | "/auth/devices/pair"
            | "/auth/sessions/exchange"
            | "/auth/sessions/refresh"
            | "/openapi.json"
            | "/docs"
    )
}

/// Scope a third-party API token must hold to call `path`. Session tokens skip this.
pub fn required_scope_for_path(path: &str) -> Option<&'static str> {
    if is_public_path(path) {
        return None;
    }
    if path.starts_with("/admin") {
        return Some("admin");
    }
    if path.starts_with("/pos") {
        return Some("pos");
    }
    if path.starts_with("/catalog") {
        return Some("catalog");
    }
    if path.starts_with("/orders") {
        return Some("orders");
    }
    if path.starts_with("/customers") {
        return Some("customers");
    }
    if path.starts_with("/documents") {
        return Some("documents");
    }
    if path.starts_with("/approvals") {
        return Some("approvals");
    }
    if path.starts_with("/audit") {
        return Some("audit");
    }
    if path.starts_with("/sync") {
        return Some("sync");
    }
    if path.starts_with("/webhooks") {
        return Some("webhooks");
    }
    Some("admin")
}

/// `*` and `admin` grant every route. Otherwise the required scope must be listed.
pub fn token_scopes_allow(scopes: &[String], required: &str) -> bool {
    scopes
        .iter()
        .any(|s| s == "*" || s == "admin" || s == required)
}

fn record_auth_metrics(operation: &'static str, outcome: &'static str, start: DateTime<Utc>) {
    metrics::counter!(AUTH_REQUESTS_TOTAL, "operation" => operation, "outcome" => outcome)
        .increment(1);
    let elapsed = (Utc::now() - start).num_milliseconds() as f64 / 1000.0;
    metrics::histogram!(AUTH_REQUEST_DURATION_SECONDS, "operation" => operation).record(elapsed);
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
        Err(_) => match authenticate_api_token(&app, &token, req.uri().path()).await {
            Ok(principal) => {
                req.extensions_mut().insert(principal);
                return next.run(req).await;
            }
            Err(status) => return status.into_response(),
        },
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
        register_id: device.register_id,
    });
    next.run(req).await
}

async fn authenticate_api_token(
    app: &AppState,
    token: &str,
    path: &str,
) -> Result<AuthPrincipal, StatusCode> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_aud = false;
    validation.validate_exp = true;
    let claims = decode::<crate::admin_api::ApiTokenClaims>(
        token,
        &DecodingKey::from_secret(app.auth.session_signing_secret.as_bytes()),
        &validation,
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?
    .claims;
    let token_id = Uuid::parse_str(&claims.sub).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let Some(stored) = crate::admin_api::load_api_token(&app.pool, token_id).await else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if stored.revoked {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if let Some(required) = required_scope_for_path(path) {
        if !token_scopes_allow(&stored.scopes, required) {
            record_auth_metrics("api_token", "forbidden", Utc::now());
            return Err(StatusCode::FORBIDDEN);
        }
    }
    record_auth_metrics("api_token", "ok", Utc::now());
    Ok(AuthPrincipal {
        session_id: token_id,
        associate_id: format!("api-token:{}", stored.name),
        device_id: Uuid::nil(),
        store_id: app.store_id,
        register_id: Some(app.register_id),
    })
}

/// Pairing codes turn an unknown device into a trusted register, so minting one needs
/// either the hub machine itself (loopback peer) or an API token with the `pairing` scope.
/// An unknown peer (no connection info) is not treated as loopback.
async fn may_mint_pairing_code(
    app: &AppState,
    peer: Option<std::net::SocketAddr>,
    headers: &axum::http::HeaderMap,
) -> bool {
    if peer.is_some_and(|addr| addr.ip().is_loopback()) {
        return true;
    }
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
    else {
        return false;
    };
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_aud = false;
    validation.validate_exp = true;
    let Ok(claims) = decode::<crate::admin_api::ApiTokenClaims>(
        token,
        &DecodingKey::from_secret(app.auth.session_signing_secret.as_bytes()),
        &validation,
    ) else {
        return false;
    };
    let Ok(token_id) = Uuid::parse_str(&claims.claims.sub) else {
        return false;
    };
    match crate::admin_api::load_api_token(&app.pool, token_id).await {
        Some(stored) => !stored.revoked && token_scopes_allow(&stored.scopes, "pairing"),
        None => false,
    }
}

pub async fn create_pairing_code(
    State(app): State<AppState>,
    connect_info: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AuthCreatePairingCodeRequest>,
) -> Result<Json<AuthCreatePairingCodeResponse>, StatusCode> {
    let start = Utc::now();
    if !app.auth.enabled {
        record_auth_metrics("pairing_codes_create", "disabled", start);
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if !may_mint_pairing_code(&app, connect_info.map(|c| c.0), &headers).await {
        record_auth_metrics("pairing_codes_create", "forbidden", start);
        return Err(StatusCode::FORBIDDEN);
    }
    if req.store_id != app.store_id {
        record_auth_metrics("pairing_codes_create", "store_mismatch", start);
        return Err(StatusCode::BAD_REQUEST);
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
        metrics::counter!(DEVICE_PAIRINGS_TOTAL, "outcome" => "invalid_code").increment(1);
        record_auth_metrics("devices_pair", "invalid_code", start);
        return Err(StatusCode::BAD_REQUEST);
    };
    if pairing.store_id != req.store_id || req.store_id != app.store_id {
        metrics::counter!(DEVICE_PAIRINGS_TOTAL, "outcome" => "store_mismatch").increment(1);
        record_auth_metrics("devices_pair", "store_mismatch", start);
        return Err(StatusCode::BAD_REQUEST);
    }
    if pairing.consumed_at.is_some()
        || pairing.expires_at < Utc::now()
        || pairing.attempts >= pairing.max_attempts
    {
        let _ = increment_pairing_code_attempts(&app.pool, pairing.id).await;
        metrics::counter!(DEVICE_PAIRINGS_TOTAL, "outcome" => "expired_or_consumed").increment(1);
        record_auth_metrics("devices_pair", "expired_or_consumed", start);
        return Err(StatusCode::BAD_REQUEST);
    }
    let device_id = Uuid::new_v4();
    let device_secret = format!("dev-{}-{}", Uuid::new_v4(), Uuid::new_v4());
    let register_id = req.register_id.unwrap_or(app.register_id);
    create_trusted_device(
        &app.pool,
        device_id,
        req.store_id,
        register_id,
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
    metrics::counter!(DEVICE_PAIRINGS_TOTAL, "outcome" => "ok").increment(1);
    record_auth_metrics("devices_pair", "ok", start);
    Ok(Json(AuthDevicePairResponse {
        device_id,
        device_secret,
        register_id,
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
    if store_id != app.store_id {
        record_auth_metrics("sessions_exchange", "store_mismatch", start);
        return Err(StatusCode::UNAUTHORIZED);
    }
    let device = get_trusted_device(&app.pool, req.device_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if device.store_id != store_id
        || device.secret_hash != hash_secret(&req.device_secret)
        || device.status != "active"
        || device.revoked_at.is_some()
    {
        metrics::counter!(AUTH_SESSIONS_TOTAL, "outcome" => "untrusted_device").increment(1);
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
    metrics::counter!(AUTH_SESSIONS_TOTAL, "outcome" => "issued").increment(1);
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
        metrics::counter!(AUTH_SESSIONS_TOTAL, "outcome" => "refresh_denied").increment(1);
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
    metrics::counter!(AUTH_SESSIONS_TOTAL, "outcome" => "refreshed").increment(1);
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
    metrics::counter!(AUTH_SESSIONS_TOTAL, "outcome" => "revoked").increment(1);
    record_auth_metrics("sessions_revoke", "ok", start);
    Ok(Json(AuthSessionRevokeResponse { revoked: true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_is_on_when_the_env_flag_is_unset() {
        assert!(parse_enabled_flag(None, true));
        assert!(!parse_enabled_flag(Some("0"), true));
        assert!(!parse_enabled_flag(Some("false"), true));
        assert!(!parse_enabled_flag(Some("no"), true));
        assert!(parse_enabled_flag(Some("1"), false));
        assert!(parse_enabled_flag(Some("TRUE"), false));
        assert!(parse_enabled_flag(Some("yes"), false));
    }

    #[test]
    fn api_token_scopes_match_the_route_they_were_issued_for() {
        assert_eq!(
            required_scope_for_path("/catalog/products"),
            Some("catalog")
        );
        assert_eq!(required_scope_for_path("/pos/command"), Some("pos"));
        assert_eq!(required_scope_for_path("/admin/api-tokens"), Some("admin"));
        assert_eq!(required_scope_for_path("/health"), None);
        let catalog = vec!["catalog".to_string()];
        assert!(token_scopes_allow(&catalog, "catalog"));
        assert!(!token_scopes_allow(&catalog, "pos"));
        assert!(token_scopes_allow(&["*".into()], "admin"));
        assert!(token_scopes_allow(&["admin".into()], "pos"));
    }
}
