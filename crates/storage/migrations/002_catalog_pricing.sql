-- Catalog items (synced or seeded for tests)
CREATE TABLE IF NOT EXISTS catalog_items (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    sku TEXT NOT NULL,
    name TEXT NOT NULL,
    category_id TEXT NOT NULL,
    tax_category_id TEXT NOT NULL,
    UNIQUE(store_id, sku)
);

-- Price book entries per store
CREATE TABLE IF NOT EXISTS price_book_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    store_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    modifier_option_id TEXT,
    price_cents INTEGER NOT NULL,
    currency TEXT NOT NULL DEFAULT 'USD'
);

-- Tax rules per store
CREATE TABLE IF NOT EXISTS tax_rules (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    tax_category_id TEXT NOT NULL,
    rate_bps INTEGER NOT NULL,
    name TEXT NOT NULL,
    inclusive INTEGER NOT NULL DEFAULT 0
);

-- Promotions (full payload as JSON)
CREATE TABLE IF NOT EXISTS promotions (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    data TEXT NOT NULL
);

-- Customers (for search and cart association)
CREATE TABLE IF NOT EXISTS customers (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    code TEXT NOT NULL,
    name TEXT NOT NULL,
    UNIQUE(store_id, code)
);
