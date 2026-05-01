-- Third-party API tokens and inbound webhook receipts.

CREATE TABLE IF NOT EXISTS api_tokens (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    scopes_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    revoked_at TEXT
);

CREATE TABLE IF NOT EXISTS inbound_webhooks (
    id TEXT PRIMARY KEY NOT NULL,
    connector_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    received_at TEXT NOT NULL,
    status TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS inbound_webhooks_connector_idx ON inbound_webhooks (connector_id, received_at);
