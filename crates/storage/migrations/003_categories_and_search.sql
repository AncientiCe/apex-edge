-- Categories (for catalog browsing and filtering)
CREATE TABLE IF NOT EXISTS categories (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    name TEXT NOT NULL,
    UNIQUE(store_id, id)
);
