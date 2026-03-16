CREATE TABLE IF NOT EXISTS trusted_devices (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    device_name TEXT NOT NULL,
    platform TEXT,
    secret_hash TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    enrolled_at TEXT NOT NULL,
    last_seen_at TEXT,
    revoked_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_trusted_devices_store_id
    ON trusted_devices (store_id);

CREATE TABLE IF NOT EXISTS device_pairing_codes (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    code_hash TEXT NOT NULL UNIQUE,
    created_by TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    consumed_at TEXT,
    consumed_device_id TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_device_pairing_codes_expires_at
    ON device_pairing_codes (expires_at);

CREATE TABLE IF NOT EXISTS auth_sessions (
    session_id TEXT PRIMARY KEY NOT NULL,
    associate_id TEXT NOT NULL,
    store_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    access_exp TEXT NOT NULL,
    refresh_exp TEXT NOT NULL,
    revoked_at TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_auth_sessions_device_id
    ON auth_sessions (device_id);

CREATE TABLE IF NOT EXISTS associate_identities (
    associate_id TEXT NOT NULL,
    store_id TEXT NOT NULL,
    name TEXT,
    email TEXT,
    claims_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (associate_id, store_id)
);
