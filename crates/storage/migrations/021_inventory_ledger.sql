-- Real-time inventory ledger: edge-owned available-to-sell tracking.
-- HQ remains authoritative for on-hand (hq_baseline_qty); the edge layers
-- reservations, committed sales, and local deltas on top to prevent oversell
-- across registers between periodic HQ syncs.

CREATE TABLE IF NOT EXISTS inventory_state (
    store_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    hq_baseline_qty INTEGER NOT NULL DEFAULT 0,
    reserved_qty INTEGER NOT NULL DEFAULT 0,
    sold_since_sync_qty INTEGER NOT NULL DEFAULT 0,
    local_adjust_qty INTEGER NOT NULL DEFAULT 0,
    baseline_as_of TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (store_id, item_id)
);

-- Per-line stock reservations held by open carts. state: active | released | committed.
CREATE TABLE IF NOT EXISTS stock_reservations (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT NOT NULL,
    cart_id TEXT NOT NULL,
    line_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    qty INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'active',
    expires_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS stock_reservations_cart_idx ON stock_reservations (store_id, cart_id, state);
CREATE INDEX IF NOT EXISTS stock_reservations_line_idx ON stock_reservations (store_id, line_id);
CREATE INDEX IF NOT EXISTS stock_reservations_expiry_idx ON stock_reservations (state, expires_at);
