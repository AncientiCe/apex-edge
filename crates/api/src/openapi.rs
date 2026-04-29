//! Minimal OpenAPI 3.1 spec served at `/openapi.json` + a Swagger UI stub at `/docs`.
//!
//! The spec is hand-authored (no `utoipa` macro dependency) so we can ship it without
//! instrumenting every handler, and extended incrementally. The ground truth is the
//! router in `apex-edge/src/app.rs`; keep them in sync and add a CI diff check against
//! `docs/openapi.golden.json`.

use axum::response::{Html, Json};

pub const OPENAPI_VERSION: &str = "3.1.0";

fn spec() -> serde_json::Value {
    serde_json::json!({
        "openapi": OPENAPI_VERSION,
        "info": {
            "title": "ApexEdge",
            "version": "0.7.0",
            "summary": "Store hub orchestrator: POS/MPOS <-> ApexEdge <-> HQ.",
            "description": "Offline-first, contract-driven retail orchestrator. Returns, till/shift, supervisor approvals, tamper-evident audit, real-time POS push, HA-ready.",
            "license": { "name": "MIT OR Apache-2.0" }
        },
        "servers": [
            { "url": "http://localhost:3000", "description": "Local hub" }
        ],
        "paths": {
            "/health": { "get": { "summary": "Liveness", "responses": { "200": { "description": "OK" } } } },
            "/ready": { "get": { "summary": "Readiness (DB probe)", "responses": { "200": { "description": "Ready" }, "503": { "description": "Not ready" } } } },
            "/pos/command": {
                "post": {
                    "summary": "POS command",
                    "description": "Idempotent cart/checkout/return/shift commands. See contracts crate for payload shapes.",
                    "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                    "responses": { "200": { "description": "PosResponseEnvelope" } }
                }
            },
            "/pos/cart/{cart_id}": { "get": { "summary": "Get cart state", "parameters": [ { "name": "cart_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "CartState" }, "404": { "description": "Not found" } } } },
            "/pos/stream": { "get": { "summary": "WebSocket real-time feed", "description": "Upgrade to WebSocket for per-store real-time events (cart, approvals, documents, sync, prices).", "responses": { "101": { "description": "Switching protocols" } } } },
            "/pos/events": { "get": { "summary": "SSE fallback for real-time feed", "responses": { "200": { "description": "text/event-stream" } } } },
            "/catalog/products": { "get": { "summary": "Search products", "responses": { "200": { "description": "Product search results" } } } },
            "/catalog/products/{id}": { "get": { "summary": "Get product by id", "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "Product" }, "404": { "description": "Not found" } } } },
            "/catalog/prices": { "get": { "summary": "Get prices for products", "parameters": [ { "name": "product_id", "in": "query", "required": true, "schema": { "type": "array", "items": { "type": "string", "format": "uuid" } } } ], "responses": { "200": { "description": "Price list" } } } },
            "/catalog/categories": { "get": { "summary": "List categories", "responses": { "200": { "description": "Category tree" } } } },
            "/customers": { "get": { "summary": "Search customers", "responses": { "200": { "description": "Customer search results" } } } },
            "/auth/pairing-codes": { "post": { "summary": "Create device pairing code", "responses": { "201": { "description": "Pairing code" }, "403": { "description": "Auth disabled or forbidden" } } } },
            "/auth/devices/pair": { "post": { "summary": "Pair a device", "responses": { "200": { "description": "Device credentials" }, "400": { "description": "Invalid code" } } } },
            "/auth/sessions/exchange": { "post": { "summary": "Exchange device credentials for session", "responses": { "200": { "description": "Session tokens" }, "401": { "description": "Unauthorized" } } } },
            "/auth/sessions/refresh": { "post": { "summary": "Refresh session token", "responses": { "200": { "description": "Session tokens" }, "401": { "description": "Unauthorized" } } } },
            "/auth/sessions/revoke": { "post": { "summary": "Revoke session", "responses": { "204": { "description": "Revoked" } } } },
            "/approvals": { "post": { "summary": "Request supervisor approval", "responses": { "202": { "description": "Pending approval created" } } } },
            "/approvals/{id}": { "get": { "summary": "Poll approval state", "responses": { "200": { "description": "ApprovalResponse" }, "404": { "description": "Not found" } } } },
            "/approvals/{id}/grant": { "post": { "summary": "Grant supervisor approval", "responses": { "200": { "description": "ApprovalResponse" } } } },
            "/approvals/{id}/deny": { "post": { "summary": "Deny supervisor approval", "responses": { "200": { "description": "ApprovalResponse" } } } },
            "/audit/verify": { "get": { "summary": "Verify audit hash chain end-to-end", "responses": { "200": { "description": "AuditChainVerification" } } } },
            "/documents/{id}": { "get": { "summary": "Fetch a generated document", "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "Document" }, "404": { "description": "Not found" } } } },
            "/orders": { "get": { "summary": "List finalized orders", "parameters": [ { "name": "shift_id", "in": "query", "required": false, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "Order summaries" } } } },
            "/orders/{id}": { "get": { "summary": "Get finalized order", "parameters": [ { "name": "id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "Order ledger entry" }, "404": { "description": "Not found" } } } },
            "/orders/{order_id}/documents": { "get": { "summary": "List documents for an order", "parameters": [ { "name": "order_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "List" } } } },
            "/orders/{order_id}/documents/gift-receipt": { "post": { "summary": "Generate gift receipt document", "parameters": [ { "name": "order_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "201": { "description": "Document" }, "404": { "description": "Source receipt not found" } } } },
            "/metrics": { "get": { "summary": "Prometheus metrics exposition", "responses": { "200": { "description": "text/plain" } } } },
            "/sync/status": { "get": { "summary": "Sync checkpoints", "responses": { "200": { "description": "Per-entity checkpoint summary" } } } },
            "/openapi.json": { "get": { "summary": "OpenAPI document", "responses": { "200": { "description": "OpenAPI JSON" } } } },
            "/docs": { "get": { "summary": "Swagger UI", "responses": { "200": { "description": "HTML documentation UI" } } } }
        },
        "components": {
            "schemas": {
                "ApprovalResponse": {
                    "type": "object",
                    "required": ["approval_id", "action", "state", "created_at", "expires_at"],
                    "properties": {
                        "approval_id": { "type": "string", "format": "uuid" },
                        "action": { "type": "string" },
                        "state": { "type": "string", "enum": ["pending", "granted", "denied", "expired"] },
                        "requested_by": { "type": "string", "nullable": true },
                        "approver_id": { "type": "string", "nullable": true },
                        "decision_reason": { "type": "string", "nullable": true },
                        "created_at": { "type": "string", "format": "date-time" },
                        "decided_at": { "type": "string", "format": "date-time", "nullable": true },
                        "expires_at": { "type": "string", "format": "date-time" }
                    }
                },
                "AuditChainVerification": {
                    "type": "object",
                    "required": ["ok", "checked"],
                    "properties": {
                        "ok": { "type": "boolean" },
                        "checked": { "type": "integer", "format": "int64" },
                        "first_bad_id": { "type": "integer", "format": "int64", "nullable": true },
                        "reason": { "type": "string", "nullable": true }
                    }
                }
            }
        }
    })
}

pub async fn openapi_handler() -> Json<serde_json::Value> {
    Json(spec())
}

pub async fn openapi_ui_handler() -> Html<&'static str> {
    Html(include_str!("../assets/swagger-ui.html"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_spec_is_well_formed_json() {
        let v = spec();
        assert_eq!(v["openapi"], OPENAPI_VERSION);
        assert_eq!(v["info"]["title"], "ApexEdge");
        assert!(v["paths"]["/pos/command"]["post"].is_object());
        assert!(v["paths"]["/audit/verify"]["get"].is_object());
        assert!(v["paths"]["/pos/stream"]["get"].is_object());
        assert_eq!(v["info"]["version"], "0.7.0");
        assert!(v["paths"]["/catalog/prices"]["get"].is_object());
        assert!(v["paths"]["/auth/pairing-codes"]["post"].is_object());
        assert!(v["paths"]["/orders"]["get"].is_object());
        assert!(v["paths"]["/orders/{id}"]["get"].is_object());
        assert!(v["paths"]["/docs"]["get"].is_object());
    }
}
