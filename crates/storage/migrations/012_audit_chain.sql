-- Tamper-evident, hash-chained audit log.
-- See migrations.rs for idempotent column-exists application (ALTERs are not
-- idempotent in SQLite on their own).
--
-- Every new row stores:
--   prev_hash   = the `hash` of the previous row (or empty if first)
--   hash        = HMAC-SHA256(hub_key, prev_hash || event_type || entity_id || payload || created_at)
--   hub_key_id  = logical id of the hub signing key (enables rotation)

-- Marker-only. Real ALTERs are applied idempotently from Rust.
CREATE TABLE IF NOT EXISTS audit_chain_marker (id INTEGER PRIMARY KEY CHECK (id = 1));

CREATE INDEX IF NOT EXISTS audit_log_created_at_idx ON audit_log (created_at);
CREATE INDEX IF NOT EXISTS audit_log_id_idx ON audit_log (id);
