-- Returns & Refunds schema.
--
-- A return is a negative-total order rooted in an optional original_order_id (for
-- receipted returns; NULL means blind return, gated by approval). return_lines mirror
-- the original cart_lines with negative effective quantity/amounts. refunds capture how
-- the customer was reimbursed.

CREATE TABLE IF NOT EXISTS returns (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT NOT NULL,
    shift_id TEXT,
    original_order_id TEXT,
    reason_code TEXT,
    state TEXT NOT NULL DEFAULT 'open',
    total_cents INTEGER NOT NULL DEFAULT 0,
    tax_cents INTEGER NOT NULL DEFAULT 0,
    refunded_cents INTEGER NOT NULL DEFAULT 0,
    approval_id TEXT,
    created_at TEXT NOT NULL,
    finalized_at TEXT
);

CREATE TABLE IF NOT EXISTS return_lines (
    id TEXT PRIMARY KEY NOT NULL,
    return_id TEXT NOT NULL,
    original_line_id TEXT,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    quantity INTEGER NOT NULL,
    unit_price_cents INTEGER NOT NULL,
    line_total_cents INTEGER NOT NULL,
    tax_cents INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS refunds (
    id TEXT PRIMARY KEY NOT NULL,
    return_id TEXT NOT NULL,
    tender_type TEXT NOT NULL,
    amount_cents INTEGER NOT NULL,
    external_reference TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS returns_store_state_idx ON returns (store_id, state);
CREATE INDEX IF NOT EXISTS returns_original_order_idx ON returns (original_order_id);
CREATE INDEX IF NOT EXISTS return_lines_return_idx ON return_lines (return_id);
CREATE INDEX IF NOT EXISTS refunds_return_idx ON refunds (return_id);
