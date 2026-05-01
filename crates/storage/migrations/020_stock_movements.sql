-- Local stock operation ledger.

CREATE TABLE IF NOT EXISTS stock_movements (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    quantity_delta INTEGER NOT NULL,
    reason TEXT NOT NULL,
    reference TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS stock_movements_item_idx ON stock_movements (store_id, item_id, created_at);
