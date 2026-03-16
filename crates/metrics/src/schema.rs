//! Metric names and label keys/values. Cardinality is bounded; do not add raw IDs as labels.

// ---------- HTTP layer (middleware) ----------
/// Counter: total requests. Labels: method, route, status_class.
pub const HTTP_REQUESTS_TOTAL: &str = "apex_edge_http_requests_total";
/// Histogram: request duration in seconds. Labels: method, route.
pub const HTTP_REQUEST_DURATION_SECONDS: &str = "apex_edge_http_request_duration_seconds";
/// Gauge: in-flight requests. Labels: route.
pub const HTTP_REQUESTS_IN_FLIGHT: &str = "apex_edge_http_requests_in_flight";

/// Normalized route template for labels (e.g. "/documents/:id" -> "documents_id").
pub fn route_label(path: &str) -> &'static str {
    match path {
        "/health" => "health",
        "/ready" => "ready",
        "/pos/command" => "pos_command",
        "/pos/cart/:cart_id" | "/pos/cart/{cart_id}" => "pos_cart",
        "/catalog/products" => "catalog_products",
        "/catalog/products/:id" | "/catalog/products/{id}" => "catalog_product_id",
        "/catalog/categories" => "catalog_categories",
        "/customers" => "customers",
        "/documents/:id" | "/documents/{id}" => "documents_id",
        "/orders/:order_id/documents" | "/orders/{order_id}/documents" => "orders_documents",
        "/orders/:order_id/documents/gift-receipt"
        | "/orders/{order_id}/documents/gift-receipt" => "orders_gift_receipt",
        "/metrics" => "metrics",
        "/sync/status" => "sync_status",
        "/auth/pairing-codes" => "auth_pairing_codes",
        "/auth/devices/pair" => "auth_devices_pair",
        "/auth/sessions/exchange" => "auth_sessions_exchange",
        "/auth/sessions/refresh" => "auth_sessions_refresh",
        "/auth/sessions/revoke" => "auth_sessions_revoke",
        _ => "unknown",
    }
}

/// Maps actual request path to a bounded route label for metrics (avoids high cardinality).
pub fn request_path_to_route(path: &str) -> &'static str {
    if path == "/health" {
        return "health";
    }
    if path == "/ready" {
        return "ready";
    }
    if path == "/pos/command" {
        return "pos_command";
    }
    if path == "/metrics" {
        return "metrics";
    }
    if path == "/catalog/products" {
        return "catalog_products";
    }
    if path == "/catalog/categories" {
        return "catalog_categories";
    }
    if path == "/customers" {
        return "customers";
    }
    if path == "/sync/status" {
        return "sync_status";
    }
    if path == "/auth/pairing-codes" {
        return "auth_pairing_codes";
    }
    if path == "/auth/devices/pair" {
        return "auth_devices_pair";
    }
    if path == "/auth/sessions/exchange" {
        return "auth_sessions_exchange";
    }
    if path == "/auth/sessions/refresh" {
        return "auth_sessions_refresh";
    }
    if path == "/auth/sessions/revoke" {
        return "auth_sessions_revoke";
    }
    if path.starts_with("/pos/cart/") && path.len() > 10 {
        return "pos_cart";
    }
    if path.starts_with("/documents/") && path.len() > 10 {
        return "documents_id";
    }
    if path.starts_with("/catalog/products/") && path.len() > 18 {
        return "catalog_product_id";
    }
    if path.starts_with("/orders/") && path.ends_with("/documents/gift-receipt") {
        return "orders_gift_receipt";
    }
    if path.starts_with("/orders/") && path.ends_with("/documents") {
        return "orders_documents";
    }
    "unknown"
}

/// status_class: 2xx, 4xx, 5xx. Use for HTTP_REQUESTS_TOTAL.
pub fn status_class(code: u16) -> &'static str {
    match code {
        200..=299 => "2xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

// ---------- POS command (api::pos) ----------
/// Counter: POS commands by operation and outcome. Labels: operation, outcome.
pub const POS_COMMANDS_TOTAL: &str = "apex_edge_pos_commands_total";
/// Histogram: POS command handler duration. Labels: operation.
pub const POS_COMMAND_DURATION_SECONDS: &str = "apex_edge_pos_command_duration_seconds";
/// Histogram: request body size in bytes (optional). No high-cardinality labels.
pub const POS_COMMAND_PAYLOAD_BYTES: &str = "apex_edge_pos_command_payload_bytes";

/// outcome: success, validation_error, unsupported_version, domain_error.
pub const OUTCOME_SUCCESS: &str = "success";
pub const OUTCOME_VALIDATION_ERROR: &str = "validation_error";
pub const OUTCOME_UNSUPPORTED_VERSION: &str = "unsupported_version";
pub const OUTCOME_DOMAIN_ERROR: &str = "domain_error";

// ---------- Documents (api::documents) ----------
/// Counter: document operations. Labels: operation, outcome.
pub const DOCUMENT_OPERATIONS_TOTAL: &str = "apex_edge_document_operations_total";
/// Histogram: document operation duration. Labels: operation.
pub const DOCUMENT_OPERATION_DURATION_SECONDS: &str =
    "apex_edge_document_operation_duration_seconds";

/// operation: get_document, list_order_documents.
pub const OP_GET_DOCUMENT: &str = "get_document";
pub const OP_LIST_ORDER_DOCUMENTS: &str = "list_order_documents";
/// outcome: hit, not_found, error.
pub const OUTCOME_HIT: &str = "hit";
pub const OUTCOME_NOT_FOUND: &str = "not_found";
pub const OUTCOME_ERROR: &str = "error";

// ---------- Outbox (outbox::dispatcher) ----------
/// Counter: dispatch attempts. Labels: outcome.
pub const OUTBOX_DISPATCH_ATTEMPTS_TOTAL: &str = "apex_edge_outbox_dispatch_attempts_total";
/// Histogram: HQ HTTP call duration in seconds.
pub const OUTBOX_DISPATCH_DURATION_SECONDS: &str = "apex_edge_outbox_dispatch_duration_seconds";
/// Counter: messages moved to DLQ.
pub const OUTBOX_DLQ_TOTAL: &str = "apex_edge_outbox_dlq_total";
/// Counter: background dispatcher loop cycles. Labels: outcome (success, error).
pub const OUTBOX_DISPATCHER_CYCLES_TOTAL: &str = "apex_edge_outbox_dispatcher_cycles_total";

/// outcome: accepted, rejected, http_error, timeout, dlq.
pub const OUTCOME_ACCEPTED: &str = "accepted";
pub const OUTCOME_REJECTED: &str = "rejected";
pub const OUTCOME_HTTP_ERROR: &str = "http_error";
pub const OUTCOME_TIMEOUT: &str = "timeout";
pub const OUTCOME_DLQ: &str = "dlq";

// ---------- Sync ingest (sync::ingest) ----------
/// Counter: ingest batches. Labels: entity, outcome.
pub const SYNC_INGEST_BATCHES_TOTAL: &str = "apex_edge_sync_ingest_batches_total";
/// Histogram: batch processing duration. Labels: entity.
pub const SYNC_INGEST_DURATION_SECONDS: &str = "apex_edge_sync_ingest_duration_seconds";

/// outcome: checkpoint_advanced, invalid_payload, conflict.
pub const OUTCOME_CHECKPOINT_ADVANCED: &str = "checkpoint_advanced";
pub const OUTCOME_INVALID_PAYLOAD: &str = "invalid_payload";
pub const OUTCOME_CONFLICT: &str = "conflict";

// ---------- Dependencies (DB, outbound HTTP) ----------
/// Counter: DB operations. Labels: operation, outcome.
pub const DB_OPERATIONS_TOTAL: &str = "apex_edge_db_operations_total";
/// DB outcome: success, error.
pub const DB_OUTCOME_SUCCESS: &str = "success";
pub const DB_OUTCOME_ERROR: &str = "error";
/// Histogram: DB operation duration in seconds. Labels: operation.
pub const DB_OPERATION_DURATION_SECONDS: &str = "apex_edge_db_operation_duration_seconds";

/// Counter: outbound HTTP calls (e.g. to HQ). Labels: status_class, outcome.
pub const DEPENDENCY_HTTP_REQUESTS_TOTAL: &str = "apex_edge_dependency_http_requests_total";
/// Histogram: outbound HTTP duration. No unbounded labels.
pub const DEPENDENCY_HTTP_DURATION_SECONDS: &str = "apex_edge_dependency_http_duration_seconds";

// ---------- Catalog / stock (api::pos_handler) ----------
/// Counter: stock availability checks on add-to-cart. Labels: outcome (ok, OUT_OF_STOCK, INSUFFICIENT_STOCK).
pub const CATALOG_STOCK_CHECKS_TOTAL: &str = "apex_edge_catalog_stock_checks_total";

/// Counter: product-by-id endpoint hits. Labels: outcome (hit, not_found, error).
pub const CATALOG_PRODUCT_BY_ID_TOTAL: &str = "apex_edge_catalog_product_by_id_total";

// ---------- Document rendering (printing) ----------
/// Counter: document render attempts. Labels: document_type, outcome (ok, template_error, pdf_error).
pub const DOCUMENT_RENDER_TOTAL: &str = "apex_edge_document_render_total";
/// Histogram: render duration in seconds. Labels: document_type.
pub const DOCUMENT_RENDER_DURATION_SECONDS: &str = "apex_edge_document_render_duration_seconds";

/// outcome for document render: ok, template_error, pdf_error.
pub const OUTCOME_TEMPLATE_ERROR: &str = "template_error";
pub const OUTCOME_PDF_ERROR: &str = "pdf_error";

// ---------- Auth (api::auth) ----------
/// Counter: auth requests by operation/outcome.
pub const AUTH_REQUESTS_TOTAL: &str = "apex_edge_auth_requests_total";
/// Histogram: auth request duration.
pub const AUTH_REQUEST_DURATION_SECONDS: &str = "apex_edge_auth_request_duration_seconds";
/// Counter: auth sessions created/refreshed/revoked outcomes.
pub const AUTH_SESSIONS_TOTAL: &str = "apex_edge_auth_sessions_total";
/// Counter: device pairing outcomes.
pub const DEVICE_PAIRINGS_TOTAL: &str = "apex_edge_device_pairings_total";
