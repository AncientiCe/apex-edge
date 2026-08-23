//! Third-party API token and webhook endpoints.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pos::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateApiTokenRequest {
    pub name: String,
    pub scopes: Vec<String>,
    pub ttl_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CreateApiTokenResponse {
    pub id: Uuid,
    pub token: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ApiTokenClaims {
    pub sub: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub exp: usize,
}

pub(crate) struct StoredApiToken {
    pub name: String,
    pub scopes: Vec<String>,
    pub revoked: bool,
}

pub(crate) async fn load_api_token(
    pool: &sqlx::SqlitePool,
    token_id: Uuid,
) -> Option<StoredApiToken> {
    let row = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT name, scopes_json, revoked_at FROM api_tokens WHERE id = ?",
    )
    .bind(token_id.to_string())
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    let scopes: Vec<String> = serde_json::from_str(&row.1).unwrap_or_default();
    Some(StoredApiToken {
        name: row.0,
        scopes,
        revoked: row.2.is_some(),
    })
}

pub async fn create_api_token(
    State(app): State<AppState>,
    Json(request): Json<CreateApiTokenRequest>,
) -> Result<Json<CreateApiTokenResponse>, axum::http::StatusCode> {
    let token_id = Uuid::new_v4();
    let ttl = request.ttl_seconds.unwrap_or(86_400).clamp(60, 31_536_000);
    let exp = (Utc::now() + Duration::seconds(ttl)).timestamp() as usize;
    let claims = ApiTokenClaims {
        sub: token_id.to_string(),
        name: request.name.clone(),
        scopes: request.scopes.clone(),
        exp,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(app.auth.session_signing_secret.as_bytes()),
    )
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query("INSERT INTO api_tokens (id, name, scopes_json, created_at) VALUES (?, ?, ?, ?)")
        .bind(token_id.to_string())
        .bind(&request.name)
        .bind(serde_json::to_string(&request.scopes).unwrap_or_else(|_| "[]".into()))
        .bind(Utc::now().to_rfc3339())
        .execute(&app.pool)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(CreateApiTokenResponse {
        id: token_id,
        token,
        scopes: request.scopes,
    }))
}

pub async fn receive_webhook(
    State(app): State<AppState>,
    Path(connector_id): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO inbound_webhooks (id, connector_id, payload_json, received_at, status) VALUES (?, ?, ?, ?, 'accepted')",
    )
    .bind(id.to_string())
    .bind(&connector_id)
    .bind(payload.to_string())
    .bind(Utc::now().to_rfc3339())
    .execute(&app.pool)
    .await
    .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "accepted": true,
        "webhook_id": id,
        "connector_id": connector_id,
    })))
}

pub async fn export_customer_data(
    State(app): State<AppState>,
    Path(customer_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let Some(customer) = apex_edge_storage::get_customer(&app.pool, app.store_id, customer_id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
    else {
        return Err(axum::http::StatusCode::NOT_FOUND);
    };

    Ok(Json(serde_json::json!({
        "customer": {
            "id": customer.id,
            "store_id": customer.store_id,
            "code": customer.code,
            "name": customer.name,
            "email": customer.email,
        }
    })))
}

pub async fn erase_customer_data(
    State(app): State<AppState>,
    Path(customer_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let erased = apex_edge_storage::pseudonymize_customer(&app.pool, app.store_id, customer_id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    if !erased {
        return Err(axum::http::StatusCode::NOT_FOUND);
    }

    Ok(Json(serde_json::json!({
        "erased": true,
        "customer_id": customer_id,
    })))
}
