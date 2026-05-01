-- Suspended sales, register layouts, and time clock.

CREATE TABLE IF NOT EXISTS parked_carts (
    id TEXT PRIMARY KEY NOT NULL,
    cart_id TEXT NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT NOT NULL,
    note TEXT,
    cart_data TEXT NOT NULL,
    total_cents INTEGER NOT NULL,
    line_count INTEGER NOT NULL,
    parked_at TEXT NOT NULL,
    recalled_at TEXT
);

CREATE TABLE IF NOT EXISTS register_layouts (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT,
    language TEXT NOT NULL DEFAULT 'en',
    tiles_json TEXT NOT NULL DEFAULT '[]',
    version INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS time_clock_entries (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT NOT NULL,
    associate_id TEXT NOT NULL,
    clocked_in_at TEXT NOT NULL,
    clocked_out_at TEXT
);

CREATE INDEX IF NOT EXISTS parked_carts_store_register_idx ON parked_carts (store_id, register_id, recalled_at);
CREATE INDEX IF NOT EXISTS register_layouts_store_register_idx ON register_layouts (store_id, register_id, language);
CREATE INDEX IF NOT EXISTS time_clock_open_idx ON time_clock_entries (store_id, associate_id, clocked_out_at);
