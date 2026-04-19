-- Supervisor approvals: gate sensitive commands (blind returns, manual discounts over
-- threshold, paid-out over threshold, cash variance, void-after-tender, etc.).
--
-- States: pending -> granted | denied | expired.

CREATE TABLE IF NOT EXISTS approvals (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT,
    action TEXT NOT NULL,
    requested_by TEXT,
    context TEXT NOT NULL DEFAULT '{}',
    state TEXT NOT NULL DEFAULT 'pending',
    approver_id TEXT,
    decision_reason TEXT,
    created_at TEXT NOT NULL,
    decided_at TEXT,
    expires_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS approvals_store_state_idx ON approvals (store_id, state);
CREATE INDEX IF NOT EXISTS approvals_expires_at_idx ON approvals (expires_at);
