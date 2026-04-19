-- Rollback for migration 012_audit_chain.sql
--
-- Drops the marker table. The audit_log columns (prev_hash, hash, hub_key_id)
-- are *not* dropped here: SQLite cannot drop columns without rewriting the
-- table, and their presence with default '' is harmless on older binaries.
-- If a full rewrite is needed, use VACUUM INTO + recreate flow in ops.

DROP TABLE IF EXISTS audit_chain_marker;
