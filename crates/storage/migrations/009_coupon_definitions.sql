-- Coupon definitions synced from HQ.
CREATE TABLE IF NOT EXISTS coupon_definitions (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    code TEXT NOT NULL,
    promo_id TEXT NOT NULL,
    max_redemptions_total INTEGER,
    max_redemptions_per_customer INTEGER,
    valid_from TEXT NOT NULL,
    valid_until TEXT,
    version INTEGER NOT NULL,
    UNIQUE(store_id, code)
);
