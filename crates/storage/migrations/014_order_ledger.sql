-- Durable order ledger.
--
-- Finalized sales are stored locally before documents and HQ submission so POS/back-office
-- clients can query order facts and shift reports can compute cash sales from the ledger.

CREATE TABLE IF NOT EXISTS orders (
    id TEXT PRIMARY KEY NOT NULL,
    cart_id TEXT NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT NOT NULL,
    shift_id TEXT,
    state TEXT NOT NULL DEFAULT 'finalized',
    subtotal_cents INTEGER NOT NULL,
    discount_cents INTEGER NOT NULL,
    tax_cents INTEGER NOT NULL,
    total_cents INTEGER NOT NULL,
    submission_id TEXT,
    created_at TEXT NOT NULL,
    finalized_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS order_lines (
    id TEXT PRIMARY KEY NOT NULL,
    order_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    quantity INTEGER NOT NULL,
    unit_price_cents INTEGER NOT NULL,
    line_total_cents INTEGER NOT NULL,
    discount_cents INTEGER NOT NULL DEFAULT 0,
    tax_cents INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS order_payments (
    id TEXT PRIMARY KEY NOT NULL,
    order_id TEXT NOT NULL,
    tender_id TEXT NOT NULL,
    tender_type TEXT NOT NULL DEFAULT 'unknown',
    amount_cents INTEGER NOT NULL,
    external_reference TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS orders_store_shift_idx ON orders (store_id, shift_id);
CREATE INDEX IF NOT EXISTS orders_cart_idx ON orders (cart_id);
CREATE INDEX IF NOT EXISTS order_lines_order_idx ON order_lines (order_id);
CREATE INDEX IF NOT EXISTS order_payments_order_idx ON order_payments (order_id);
CREATE INDEX IF NOT EXISTS order_payments_tender_type_idx ON order_payments (tender_type);
