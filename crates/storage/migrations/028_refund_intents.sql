-- Reserve refund value before contacting a provider. Completed refunds stay in refunds.
CREATE TABLE IF NOT EXISTS refund_intents (
    id TEXT PRIMARY KEY NOT NULL,
    return_id TEXT NOT NULL,
    tender_type TEXT NOT NULL,
    amount_cents INTEGER NOT NULL CHECK(amount_cents > 0),
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS refund_intents_return_idx ON refund_intents(return_id);
