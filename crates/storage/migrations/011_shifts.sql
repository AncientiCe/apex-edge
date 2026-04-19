-- Till & Shift schema.
--
-- One shift is open per (store, register) at a time. Orders and returns are attached to
-- a shift via shift_id (logical, not FK-enforced — orders/returns tables are
-- pre-existing and we keep the link in application code + dedicated columns on return).
--
-- shift_movements record cash-in / cash-out / no-sale. shift_cash_counts record
-- close-of-drawer variance.

CREATE TABLE IF NOT EXISTS shifts (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT NOT NULL,
    associate_id TEXT,
    state TEXT NOT NULL DEFAULT 'open',
    opening_float_cents INTEGER NOT NULL DEFAULT 0,
    closing_counted_cents INTEGER,
    expected_cents INTEGER,
    variance_cents INTEGER,
    opened_at TEXT NOT NULL,
    closed_at TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS shifts_one_open_per_register
    ON shifts (store_id, register_id) WHERE state = 'open';

CREATE TABLE IF NOT EXISTS shift_movements (
    id TEXT PRIMARY KEY NOT NULL,
    shift_id TEXT NOT NULL,
    kind TEXT NOT NULL,  -- paid_in | paid_out | no_sale
    amount_cents INTEGER NOT NULL DEFAULT 0,
    reason TEXT,
    approval_id TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS shift_movements_shift_idx ON shift_movements (shift_id);

CREATE TABLE IF NOT EXISTS shift_cash_counts (
    id TEXT PRIMARY KEY NOT NULL,
    shift_id TEXT NOT NULL,
    denominations_json TEXT NOT NULL DEFAULT '{}',
    counted_cents INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS shift_cash_counts_shift_idx ON shift_cash_counts (shift_id);
