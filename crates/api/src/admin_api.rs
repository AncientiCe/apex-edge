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
struct ApiTokenClaims {
    sub: String,
    name: String,
    scopes: Vec<String>,
    exp: usize,
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
