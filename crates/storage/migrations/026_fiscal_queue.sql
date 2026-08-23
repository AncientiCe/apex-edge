-- Sign-later queue: a fiscal transaction that could not be signed at sale time
-- is written here so the till can keep selling and a background sweeper can
-- retry the signer without blocking the customer.
--
-- status:
--   pending      waiting for the next attempt
--   signed       the authority accepted it; the subject row carries the receipt
--   dead_letter  a permanent rejection; an operator has to look

CREATE TABLE IF NOT EXISTS fiscal_signing_queue (
    id TEXT PRIMARY KEY NOT NULL,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS fiscal_signing_subject_idx
    ON fiscal_signing_queue (subject_kind, subject_id);

CREATE INDEX IF NOT EXISTS fiscal_signing_due_idx
    ON fiscal_signing_queue (status, next_attempt_at);
