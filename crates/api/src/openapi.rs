//! Minimal OpenAPI 3.1 spec served at `/openapi.json` + a Swagger UI stub at `/docs`.
//!
//! The spec is hand-authored (no `utoipa` macro dependency) so we can ship it without
//! instrumenting every handler, and extended incrementally. The ground truth is the
//! router in `apex-edge/src/app.rs`; keep them in sync and add a CI diff check against
//! `docs/openapi.golden.json`.

use axum::response::{Html, Json};

pub const OPENAPI_VERSION: &str = "3.1.0";

/// Product release version shown in `info.version`. This crate is versioned independently
/// from the `apex-edge` binary, so this constant cannot be derived at compile time — it
/// must be bumped by hand alongside `apex-edge/Cargo.toml` and `CHANGELOG.md` as part of
/// every release (tracked in the release checklist).
pub const APEX_EDGE_RELEASE_VERSION: &str = "1.2.0";

fn spec() -> serde_json::Value {
    serde_json::json!({
        "openapi": OPENAPI_VERSION,
        "info": {
            "title": "ApexEdge",
            "version": APEX_EDGE_RELEASE_VERSION,
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
                    "description": "Idempotent cart/checkout/return/shift/gift-card/loyalty commands (tagged union on `action`; see contracts crate for payload shapes). Gift card commands (`issue_gift_card`, `activate_gift_card`, `reload_gift_card`, `redeem_gift_card`) return a GiftCardInfo payload, and loyalty's `earn_loyalty_points` returns a LoyaltyAccountInfo payload, except `redeem_gift_card`/`redeem_loyalty_points` which return the updated CartState after applying the tender. FinalizeOrder also auto-earns loyalty points for carts with an attached customer. `print_document` prints a previously generated document on a printer attached to the hub, and is refused when none is configured.",
                    "requestBody": { "required": true, "content": { "application/json": { "schema": { "type": "object" } } } },
                    "responses": { "200": { "description": "PosResponseEnvelope" } }
                }
            },
            "/pos/cart/{cart_id}": { "get": { "summary": "Get cart state", "parameters": [ { "name": "cart_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "CartState" }, "404": { "description": "Not found" } } } },
            "/pos/stream": { "get": { "summary": "WebSocket real-time feed", "description": "Upgrade to WebSocket for per-store real-time events (cart, approvals, documents, sync, prices, stock, presence, handoff). Pass `register_id` for presence tracking and `since` to replay missed events.", "parameters": [ { "name": "store_id", "in": "query", "required": false, "schema": { "type": "string", "format": "uuid" } }, { "name": "register_id", "in": "query", "required": false, "schema": { "type": "string", "format": "uuid" } }, { "name": "since", "in": "query", "required": false, "description": "Last seq seen; replays newer events or signals resnapshot_required.", "schema": { "type": "integer", "format": "int64" } } ], "responses": { "101": { "description": "Switching protocols" } } } },
            "/pos/events": { "get": { "summary": "SSE fallback for real-time feed", "description": "Server-Sent Events fallback. Supports the same `store_id`, `register_id`, and `since` query parameters as /pos/stream.", "parameters": [ { "name": "store_id", "in": "query", "required": false, "schema": { "type": "string", "format": "uuid" } }, { "name": "register_id", "in": "query", "required": false, "schema": { "type": "string", "format": "uuid" } }, { "name": "since", "in": "query", "required": false, "schema": { "type": "integer", "format": "int64" } } ], "responses": { "200": { "description": "text/event-stream" } } } },
            "/pos/registers": { "get": { "summary": "List present registers", "description": "Registers currently holding at least one live stream connection in the store.", "parameters": [ { "name": "store_id", "in": "query", "required": false, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "Present register ids", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RegisterPresence" } } } } } } },
            "/pos/snapshot": { "get": { "summary": "Full live store state (resnapshot)", "description": "Authoritative live state for a client that reconnected after a gap (resnapshot_required): current stock availability, present registers, open parked carts, and the latest stream seq.", "parameters": [ { "name": "store_id", "in": "query", "required": false, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "Store snapshot", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/StoreSnapshot" } } } } } } },
            "/pos/returns/lookup": { "get": { "summary": "Look up an order for return (store-wide)", "description": "Find an order anywhere in the store for a return, regardless of which register finalized it.", "parameters": [ { "name": "order_id", "in": "query", "required": true, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "Order ledger entry" }, "404": { "description": "Not found" } } } },
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
            "/admin/api-tokens": { "post": { "summary": "Create scoped third-party API token", "responses": { "200": { "description": "CreateApiTokenResponse" } } } },
            "/admin/customers/{id}/export": { "get": { "summary": "Export customer data for privacy requests", "responses": { "200": { "description": "Customer data export" }, "404": { "description": "Not found" } } } },
            "/admin/customers/{id}/erase": { "post": { "summary": "Pseudonymize customer data for privacy requests", "responses": { "200": { "description": "Customer erased" }, "404": { "description": "Not found" } } } },
            "/admin/outbox/destinations": { "get": { "summary": "Outbox destinations and how far behind each one is", "description": "Every enabled destination with its pending and dead-letter delivery counts. Queue depth per destination is what shows an integration has stopped accepting.", "responses": { "200": { "description": "DestinationSummary[]" } } } },
            "/admin/outbox/dead-letters": { "get": { "summary": "Deliveries a destination gave up on", "description": "Per-destination deliveries that exhausted their attempts, with the last error. These need an operator: they are never retried automatically.", "responses": { "200": { "description": "DeadLetterEntry[]" } } } },
            "/admin/outbox/dead-letters/{attempt_id}/retry": { "post": { "summary": "Requeue a dead-lettered delivery", "description": "Queues the delivery again from the start of its backoff, once the cause has been fixed. Reports requeued=false when the delivery was not dead-lettered.", "parameters": [ { "name": "attempt_id", "in": "path", "required": true, "schema": { "type": "string", "format": "uuid" } } ], "responses": { "200": { "description": "RetryOutcome" } } } },
            "/webhooks/{connector_id}": { "post": { "summary": "Receive connector webhook", "parameters": [ { "name": "connector_id", "in": "path", "required": true, "schema": { "type": "string" } } ], "responses": { "200": { "description": "Webhook accepted" } } } },
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
                },
                "RegisterPresence": {
                    "type": "object",
                    "required": ["store_id", "registers"],
                    "properties": {
                        "store_id": { "type": "string", "format": "uuid" },
                        "registers": { "type": "array", "items": { "type": "string", "format": "uuid" } }
                    }
                },
                "GiftCardInfo": {
                    "type": "object",
                    "required": ["gift_card_id", "code", "balance_cents", "currency", "state"],
                    "properties": {
                        "gift_card_id": { "type": "string", "format": "uuid" },
                        "code": { "type": "string" },
                        "balance_cents": { "type": "integer", "format": "int64" },
                        "currency": { "type": "string" },
                        "state": { "type": "string", "enum": ["issued", "active", "disabled"] }
                    }
                },
                "LoyaltyAccountInfo": {
                    "type": "object",
                    "required": ["customer_id", "points"],
                    "properties": {
                        "customer_id": { "type": "string", "format": "uuid" },
                        "points": { "type": "integer", "format": "int64" }
                    }
                },
                "StoreSnapshot": {
                    "type": "object",
                    "required": ["store_id", "seq", "stock", "registers", "parked_carts"],
                    "properties": {
                        "store_id": { "type": "string", "format": "uuid" },
                        "seq": { "type": "integer", "format": "int64", "description": "Latest stream sequence; resume /pos/stream with since=seq." },
                        "stock": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["item_id", "available_to_sell"],
                                "properties": {
                                    "item_id": { "type": "string", "format": "uuid" },
                                    "available_to_sell": { "type": "integer", "format": "int64" }
                                }
                            }
                        },
                        "registers": { "type": "array", "items": { "type": "string", "format": "uuid" } },
                        "parked_carts": { "type": "array", "items": { "type": "object" } }
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
        assert_eq!(v["info"]["version"], APEX_EDGE_RELEASE_VERSION);
        assert!(v["paths"]["/catalog/prices"]["get"].is_object());
        assert!(v["paths"]["/auth/pairing-codes"]["post"].is_object());
        assert!(v["paths"]["/orders"]["get"].is_object());
        assert!(v["paths"]["/orders/{id}"]["get"].is_object());
        assert!(v["paths"]["/admin/api-tokens"]["post"].is_object());
        assert!(v["paths"]["/admin/customers/{id}/export"]["get"].is_object());
        assert!(v["paths"]["/admin/customers/{id}/erase"]["post"].is_object());
        assert!(v["paths"]["/webhooks/{connector_id}"]["post"].is_object());
        assert!(v["paths"]["/admin/outbox/destinations"]["get"].is_object());
        assert!(v["paths"]["/admin/outbox/dead-letters"]["get"].is_object());
        assert!(
            v["paths"]["/admin/outbox/dead-letters/{attempt_id}/retry"]["post"].is_object(),
            "a dead-letter queue an operator cannot drain is the same as dropping the data"
        );
        assert!(v["paths"]["/admin/outbox/destinations"]["get"].is_object());
        assert!(v["paths"]["/admin/outbox/dead-letters"]["get"].is_object());
        assert!(
            v["paths"]["/admin/outbox/dead-letters/{attempt_id}/retry"]["post"].is_object(),
            "a dead-letter queue an operator cannot drain is the same as dropping the data"
        );
        assert!(v["paths"]["/docs"]["get"].is_object());
        assert!(v["components"]["schemas"]["GiftCardInfo"].is_object());
        assert!(v["components"]["schemas"]["LoyaltyAccountInfo"].is_object());
    }

    #[test]
    fn openapi_documents_realtime_coordination_endpoints() {
        let v = spec();
        // Multi-register coordination + continuity endpoints added with the Edge Store Brain.
        assert!(v["paths"]["/pos/registers"]["get"].is_object());
        assert!(v["paths"]["/pos/snapshot"]["get"].is_object());
        assert!(v["paths"]["/pos/returns/lookup"]["get"].is_object());
        // Returns lookup requires order_id.
        assert_eq!(
            v["paths"]["/pos/returns/lookup"]["get"]["parameters"][0]["name"],
            "order_id"
        );
        // Snapshot/presence response schemas are defined.
        assert!(v["components"]["schemas"]["StoreSnapshot"].is_object());
        assert!(v["components"]["schemas"]["RegisterPresence"].is_object());
    }
}
