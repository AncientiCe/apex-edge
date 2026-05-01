-- Multi-destination outbox routing for cloud connectors.

CREATE TABLE IF NOT EXISTS outbox_destinations (
    id TEXT PRIMARY KEY NOT NULL,
    code TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    endpoint TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    config_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS outbox_delivery_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    outbox_id TEXT NOT NULL,
    destination_id TEXT NOT NULL,
    status TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    next_attempt_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS outbox_delivery_destination_idx ON outbox_delivery_attempts (destination_id, status, next_attempt_at);
