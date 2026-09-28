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
        "/catalog/prices" => "catalog_prices",
        "/catalog/categories" => "catalog_categories",
        "/customers" => "customers",
        "/documents/:id" | "/documents/{id}" => "documents_id",
        "/orders" => "orders",
        "/orders/:id" | "/orders/{id}" => "orders_id",
        "/orders/:order_id/documents" | "/orders/{order_id}/documents" => "orders_documents",
        "/orders/:order_id/documents/gift-receipt"
        | "/orders/{order_id}/documents/gift-receipt" => "orders_gift_receipt",
        "/metrics" => "metrics",
        "/sync/status" => "sync_status",
        "/audit/verify" => "audit_verify",
        "/approvals" => "approvals",
        "/approvals/:id" | "/approvals/{id}" => "approvals_id",
        "/approvals/:id/grant" | "/approvals/{id}/grant" => "approvals_grant",
        "/approvals/:id/deny" | "/approvals/{id}/deny" => "approvals_deny",
        "/pos/stream" => "pos_stream",
        "/pos/events" => "pos_events",
        "/pos/registers" => "pos_registers",
        "/pos/snapshot" => "pos_snapshot",
        "/pos/returns/lookup" => "pos_returns_lookup",
        "/openapi.json" => "openapi_json",
        "/docs" => "docs",
        "/auth/pairing-codes" => "auth_pairing_codes",
        "/auth/devices/pair" => "auth_devices_pair",
        "/auth/sessions/exchange" => "auth_sessions_exchange",
        "/auth/sessions/refresh" => "auth_sessions_refresh",
        "/auth/sessions/revoke" => "auth_sessions_revoke",
        "/admin/api-tokens" => "admin_api_tokens",
        "/admin/customers/:id/export" | "/admin/customers/{id}/export" => "admin_customer_export",
        "/admin/customers/:id/erase" | "/admin/customers/{id}/erase" => "admin_customer_erase",
        "/webhooks/:connector_id" | "/webhooks/{connector_id}" => "webhooks_connector",
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
    if path == "/catalog/prices" {
        return "catalog_prices";
    }
    if path == "/catalog/categories" {
        return "catalog_categories";
    }
    if path == "/customers" {
        return "customers";
    }
    if path == "/orders" {
        return "orders";
    }
    if path == "/sync/status" {
        return "sync_status";
    }
    if path == "/audit/verify" {
        return "audit_verify";
    }
    if path == "/approvals" {
        return "approvals";
    }
    if path == "/pos/stream" {
        return "pos_stream";
    }
    if path == "/pos/events" {
        return "pos_events";
    }
    if path == "/pos/registers" {
        return "pos_registers";
    }
    if path == "/pos/snapshot" {
        return "pos_snapshot";
    }
    if path == "/pos/returns/lookup" {
        return "pos_returns_lookup";
    }
    if path == "/openapi.json" {
        return "openapi_json";
    }
    if path == "/docs" {
        return "docs";
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
    if path == "/admin/api-tokens" {
        return "admin_api_tokens";
    }
    if path.starts_with("/admin/customers/") && path.ends_with("/export") {
        return "admin_customer_export";
    }
    if path.starts_with("/admin/customers/") && path.ends_with("/erase") {
        return "admin_customer_erase";
    }
    if path.starts_with("/webhooks/") && path.len() > 10 {
        return "webhooks_connector";
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
    if path.starts_with("/approvals/") && path.ends_with("/grant") {
        return "approvals_grant";
    }
    if path.starts_with("/approvals/") && path.ends_with("/deny") {
        return "approvals_deny";
    }
    if path.starts_with("/approvals/") && path.len() > 11 {
        return "approvals_id";
    }
    if path.starts_with("/orders/") && path.ends_with("/documents/gift-receipt") {
        return "orders_gift_receipt";
    }
    if path.starts_with("/orders/") && path.ends_with("/documents") {
        return "orders_documents";
    }
    if path.starts_with("/orders/") && path.len() > 8 {
        return "orders_id";
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

// ---------- Payments (api::pos_handler + adapters) ----------
/// Counter: payment attempts by provider and outcome. Labels: provider, outcome.
pub const PAYMENT_ATTEMPTS_TOTAL: &str = "apex_edge_payment_attempts_total";
/// Histogram: payment operation duration in seconds. Labels: provider.
pub const PAYMENT_DURATION_SECONDS: &str = "apex_edge_payment_duration_seconds";
/// Counter: provider-side captures. Labels: provider, outcome.
pub const PAYMENT_CAPTURES_TOTAL: &str = "apex_edge_payment_captures_total";
/// Counter: reversals of money taken for a sale that never completed. Labels: provider, outcome.
pub const PAYMENT_REVERSALS_TOTAL: &str = "apex_edge_payment_reversals_total";
/// Gauge: captured payments still owed a reversal. Should sit at zero.
pub const PAYMENT_REVERSALS_PENDING: &str = "apex_edge_payment_reversals_pending";
/// Counter: provider-side refunds issued during returns. Labels: provider, outcome.
pub const PAYMENT_REFUNDS_TOTAL: &str = "apex_edge_payment_refunds_total";

/// Payment outcomes beyond the shared success/error pair.
pub const OUTCOME_DECLINED: &str = "declined";
pub const OUTCOME_PARTIAL: &str = "partial";
pub const OUTCOME_INDETERMINATE: &str = "indeterminate";
pub const OUTCOME_UNKNOWN_PROVIDER: &str = "unknown_provider";

// ---------- Tax providers (domain pricing + adapters) ----------
/// Counter: tax quote attempts by provider and outcome. Labels: provider, outcome.
pub const TAX_QUOTES_TOTAL: &str = "apex_edge_tax_quote_total";
/// Histogram: tax quote duration in seconds. Labels: provider.
pub const TAX_QUOTE_DURATION_SECONDS: &str = "apex_edge_tax_quote_duration_seconds";

// ---------- Hardware adapters ----------
/// Counter: hardware operations by device, operation, and outcome.
pub const HARDWARE_OPERATIONS_TOTAL: &str = "apex_edge_hardware_operations_total";
/// Histogram: hardware operation duration in seconds. Labels: device, operation.
pub const HARDWARE_OPERATION_DURATION_SECONDS: &str =
    "apex_edge_hardware_operation_duration_seconds";

// ---------- Store operations ----------
/// Counter: suspended sale/time-clock operations by operation and outcome.
pub const STORE_OPERATIONS_TOTAL: &str = "apex_edge_store_operations_total";
/// Histogram: suspended sale/time-clock operation duration in seconds. Labels: operation.
pub const STORE_OPERATION_DURATION_SECONDS: &str = "apex_edge_store_operation_duration_seconds";

// ---------- Gift cards and loyalty ----------
/// Counter: gift card operations by operation and outcome.
pub const GIFT_CARD_OPERATIONS_TOTAL: &str = "apex_edge_gift_card_operations_total";
/// Histogram: gift card operation latency in seconds, labelled by operation.
pub const GIFT_CARD_OPERATION_DURATION_SECONDS: &str =
    "apex_edge_gift_card_operation_duration_seconds";
/// Counter: loyalty operations by operation and outcome.
pub const LOYALTY_OPERATIONS_TOTAL: &str = "apex_edge_loyalty_operations_total";
/// Histogram: loyalty operation latency in seconds, labelled by operation.
pub const LOYALTY_OPERATION_DURATION_SECONDS: &str = "apex_edge_loyalty_operation_duration_seconds";

// ---------- Cloud connectors ----------
/// Counter: cloud connector deliveries by connector and outcome.
pub const CLOUD_CONNECTOR_DELIVERIES_TOTAL: &str = "apex_edge_cloud_connector_deliveries_total";
/// Histogram: cloud connector delivery duration in seconds. Labels: connector.
pub const CLOUD_CONNECTOR_DELIVERY_DURATION_SECONDS: &str =
    "apex_edge_cloud_connector_delivery_duration_seconds";

// ---------- Stock operations ----------
/// Counter: stock movement operations by operation and outcome.
pub const STOCK_OPERATIONS_TOTAL: &str = "apex_edge_stock_operations_total";

// ---------- Real-time inventory ledger (api::pos_handler + sync) ----------
/// Counter: reservation attempts on add-to-cart. Labels: outcome (reserved, untracked, insufficient, error).
pub const INVENTORY_RESERVATIONS_TOTAL: &str = "apex_edge_inventory_reservations_total";
/// Counter: oversell attempts prevented by the ledger (no label).
pub const INVENTORY_OVERSELL_PREVENTED_TOTAL: &str = "apex_edge_inventory_oversell_prevented_total";
/// Counter: HQ baseline rebases applied during sync. Labels: outcome (success, error).
pub const INVENTORY_RECONCILE_TOTAL: &str = "apex_edge_inventory_reconcile_total";
/// Histogram: inventory rebase/reconcile duration in seconds.
pub const INVENTORY_RECONCILE_DURATION_SECONDS: &str =
    "apex_edge_inventory_reconcile_duration_seconds";
/// Counter: detected availability drift events during reconcile (no label).
pub const INVENTORY_DRIFT_TOTAL: &str = "apex_edge_inventory_drift_total";
/// Counter: stale reservations expired by the TTL sweeper (no label).
pub const INVENTORY_RESERVATIONS_EXPIRED_TOTAL: &str =
    "apex_edge_inventory_reservations_expired_total";

// ---------- Multi-register coordination (api::stream + api::pos_handler) ----------
/// Gauge: currently-present registers per store. Label: store-scoped via process; no id label.
pub const REGISTER_PRESENCE: &str = "apex_edge_register_presence";
/// Counter: parked-cart handoff events. Labels: outcome (claimed, conflict, not_found, error).
pub const CART_HANDOFF_TOTAL: &str = "apex_edge_cart_handoff_total";

// ---------- Continuity / freshness ----------
/// Gauge: seconds since the last successful HQ sync.
pub const SYNC_STALENESS_SECONDS: &str = "apex_edge_sync_staleness_seconds";
/// Gauge: 1 when the hub is in degraded (stale-sync) mode, else 0.
pub const EDGE_DEGRADED_MODE: &str = "apex_edge_edge_degraded_mode";

// ---------- Fiscal providers ----------
/// Counter: fiscal receipt signing by provider and outcome (success, error, queued).
pub const FISCAL_RECEIPTS_TOTAL: &str = "apex_edge_fiscal_receipts_total";
/// Histogram: fiscal receipt signing latency in seconds, labelled by provider.
pub const FISCAL_RECEIPT_DURATION_SECONDS: &str = "apex_edge_fiscal_receipt_duration_seconds";
/// Gauge: unsigned fiscal transactions waiting in the sign-later queue. Labels: status.
pub const FISCAL_QUEUE_DEPTH: &str = "apex_edge_fiscal_queue_depth";
/// Counter: background sign-later attempts. Labels: provider, outcome.
pub const FISCAL_SIGN_LATER_TOTAL: &str = "apex_edge_fiscal_sign_later_total";
/// Counter: fiscal file exports. Labels: kind (dsfinvk, xrechnung, factur_x), outcome.
pub const FISCAL_EXPORTS_TOTAL: &str = "apex_edge_fiscal_exports_total";
/// outcome: queued (sale completed, signature deferred).
pub const OUTCOME_QUEUED: &str = "queued";

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
/// Counter: dispatch attempts. Labels: destination, outcome.
pub const OUTBOX_DISPATCH_ATTEMPTS_TOTAL: &str = "apex_edge_outbox_dispatch_attempts_total";
/// Histogram: delivery HTTP call duration in seconds. Labels: destination.
pub const OUTBOX_DISPATCH_DURATION_SECONDS: &str = "apex_edge_outbox_dispatch_duration_seconds";
/// Counter: deliveries given up on. Labels: destination.
pub const OUTBOX_DLQ_TOTAL: &str = "apex_edge_outbox_dlq_total";
/// Counter: background dispatcher loop cycles. Labels: outcome (success, error).
pub const OUTBOX_DISPATCHER_CYCLES_TOTAL: &str = "apex_edge_outbox_dispatcher_cycles_total";
/// Gauge: deliveries waiting per state. Labels: state (pending, dead_letter).
///
/// Queue depth is the number an operator actually watches: a destination that has stopped
/// accepting shows up here long before anyone notices missing data downstream.
pub const OUTBOX_QUEUE_DEPTH: &str = "apex_edge_outbox_queue_depth";
/// Counter: submissions fanned out to a destination for the first time. Labels: destination.
pub const OUTBOX_FANOUT_TOTAL: &str = "apex_edge_outbox_fanout_total";
/// Counter: deliveries skipped because the destination does not want that payload kind.
/// Labels: destination, kind.
pub const OUTBOX_FILTERED_TOTAL: &str = "apex_edge_outbox_filtered_total";
/// Counter: HMAC signing of webhook deliveries. Labels: destination, outcome (signed,
/// secret_missing).
pub const OUTBOX_SIGNING_TOTAL: &str = "apex_edge_outbox_signing_total";
pub const OUTCOME_SIGNED: &str = "signed";
pub const OUTCOME_SECRET_MISSING: &str = "secret_missing";

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
/// Counter: catalog prices endpoint requests. Labels: outcome (hit, empty, error).
pub const CATALOG_PRICES_TOTAL: &str = "apex_edge_catalog_prices_total";
/// Counter: source used to shape product `image_urls`. Labels: source (inventory, catalog, placeholder).
pub const CATALOG_PRODUCT_IMAGE_SELECTION_TOTAL: &str =
    "apex_edge_catalog_product_image_selection_total";

// ---------- Document rendering (printing) ----------
/// Counter: document render attempts. Labels: document_type, outcome (ok, template_error, pdf_error).
pub const DOCUMENT_RENDER_TOTAL: &str = "apex_edge_document_render_total";
/// Histogram: render duration in seconds. Labels: document_type.
pub const DOCUMENT_RENDER_DURATION_SECONDS: &str = "apex_edge_document_render_duration_seconds";

/// outcome for document render: ok, template_error, pdf_error.
pub const OUTCOME_TEMPLATE_ERROR: &str = "template_error";
pub const OUTCOME_PDF_ERROR: &str = "pdf_error";

// ---------- Audit chain (storage::audit + api::audit) ----------
/// Counter: audit chain verifications. Labels: outcome (ok, broken, error).
pub const AUDIT_CHAIN_VERIFICATIONS_TOTAL: &str = "apex_edge_audit_chain_verifications_total";
/// Gauge: current audit chain length (rows verified last).
pub const AUDIT_CHAIN_LENGTH: &str = "apex_edge_audit_chain_length";
/// Counter: audit records appended. Labels: outcome (success, error).
pub const AUDIT_RECORDS_TOTAL: &str = "apex_edge_audit_records_total";

// ---------- Approvals (api::approvals) ----------
/// Counter: supervisor approval events. Labels: action, outcome (requested, granted, denied, expired).
pub const APPROVALS_TOTAL: &str = "apex_edge_approvals_total";
/// Histogram: time between request and grant/deny in seconds.
pub const APPROVAL_WAIT_DURATION_SECONDS: &str = "apex_edge_approval_wait_duration_seconds";

// ---------- Real-time POS stream (api::stream) ----------
/// Gauge: active WS/SSE connections.
pub const STREAM_CONNECTIONS: &str = "apex_edge_stream_connections";
/// Counter: messages broadcast. Labels: kind.
pub const STREAM_MESSAGES_TOTAL: &str = "apex_edge_stream_messages_total";

// ---------- Returns & Refunds (api::returns) ----------
/// Counter: completed/attempted returns. Labels: outcome (success, rejected, error).
pub const RETURNS_TOTAL: &str = "apex_edge_returns_total";
/// Histogram: return handler duration in seconds.
pub const RETURN_DURATION_SECONDS: &str = "apex_edge_return_duration_seconds";
/// Counter: refund tender events. Labels: tender_type, outcome.
pub const REFUND_TENDER_TOTAL: &str = "apex_edge_refund_tender_total";

// ---------- Till & Shifts (api::shifts) ----------
/// Counter: shift lifecycle events. Labels: outcome (opened, closed, rejected, error).
pub const SHIFTS_TOTAL: &str = "apex_edge_shifts_total";
/// Histogram: cash-count variance in cents (absolute).
pub const SHIFT_VARIANCE_CENTS: &str = "apex_edge_shift_variance_cents";
/// Counter: cash drawer movements. Labels: kind, outcome.
pub const CASH_MOVEMENTS_TOTAL: &str = "apex_edge_cash_movements_total";

// ---------- Order ledger (api::orders / api::pos_handler) ----------
/// Counter: finalized order ledger writes. Labels: outcome (success, error).
pub const ORDERS_FINALIZED_TOTAL: &str = "apex_edge_orders_finalized_total";
/// Counter: order lookup/list requests. Labels: operation, outcome (hit, not_found, error).
pub const ORDERS_LOOKUP_TOTAL: &str = "apex_edge_orders_lookup_total";
/// Histogram: order ledger write duration in seconds.
pub const ORDERS_LEDGER_WRITE_DURATION_SECONDS: &str =
    "apex_edge_orders_ledger_write_duration_seconds";

// ---------- Role / HA ----------
/// Gauge: current hub role (1 = role-active). Labels: role (primary, standby).
pub const ROLE_GAUGE: &str = "apex_edge_role";
/// Gauge: WAL replication lag (seconds), reported by Litestream sidecar or a probe.
pub const WAL_REPLICATION_LAG_SECONDS: &str = "apex_edge_wal_replication_lag_seconds";

// ---------- Synthetic journey (make smoke-loop) ----------
/// Counter: synthetic journey runs. Labels: outcome.
pub const SYNTHETIC_JOURNEY_TOTAL: &str = "apex_edge_synthetic_journey_total";
/// Histogram: synthetic journey duration in seconds.
pub const SYNTHETIC_JOURNEY_DURATION_SECONDS: &str = "apex_edge_synthetic_journey_duration_seconds";

// ---------- Auth (api::auth) ----------
/// Counter: auth requests by operation/outcome.
pub const AUTH_REQUESTS_TOTAL: &str = "apex_edge_auth_requests_total";
/// Histogram: auth request duration.
pub const AUTH_REQUEST_DURATION_SECONDS: &str = "apex_edge_auth_request_duration_seconds";
/// Counter: auth sessions created/refreshed/revoked outcomes.
pub const AUTH_SESSIONS_TOTAL: &str = "apex_edge_auth_sessions_total";
/// Counter: device pairing outcomes.
pub const DEVICE_PAIRINGS_TOTAL: &str = "apex_edge_device_pairings_total";

// ---------- Rate limiting (api::rate_limit) ----------
/// Counter: rate-limit decisions. Labels: bucket (auth, pos), outcome (allowed, rejected).
pub const RATE_LIMIT_DECISIONS_TOTAL: &str = "apex_edge_rate_limit_decisions_total";
/// Counter: rejected requests. Labels: bucket.
pub const RATE_LIMIT_REJECTED_TOTAL: &str = "apex_edge_rate_limit_rejected_total";

// ---------- TLS listener (apex-edge binary) ----------
/// Gauge: 1 when the hub is serving HTTPS. Labels: client_auth (off, required).
pub const TLS_ENABLED: &str = "apex_edge_tls_enabled";

// ---------- CORS (apex-edge binary) ----------
/// Gauge: 1 for the active CORS policy. Labels: mode (allow_list, localhost_only).
pub const CORS_MODE: &str = "apex_edge_cors_mode";

// ---------- Auth signing secret (apex-edge binary) ----------
/// Gauge: 1 for where the session signing secret came from at boot. Labels: source (env,
/// file_loaded, file_generated).
pub const AUTH_SIGNING_SECRET_SOURCE: &str = "apex_edge_auth_signing_secret_source";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_class_routes_have_bounded_labels() {
        for (path, expected) in [
            ("/catalog/prices", "catalog_prices"),
            ("/audit/verify", "audit_verify"),
            ("/approvals", "approvals"),
            (
                "/approvals/550e8400-e29b-41d4-a716-446655440000",
                "approvals_id",
            ),
            (
                "/approvals/550e8400-e29b-41d4-a716-446655440000/grant",
                "approvals_grant",
            ),
            ("/pos/stream", "pos_stream"),
            ("/pos/events", "pos_events"),
            ("/pos/snapshot", "pos_snapshot"),
            ("/orders", "orders"),
            ("/orders/550e8400-e29b-41d4-a716-446655440000", "orders_id"),
            ("/openapi.json", "openapi_json"),
            ("/docs", "docs"),
        ] {
            assert_eq!(request_path_to_route(path), expected);
        }
    }
}
