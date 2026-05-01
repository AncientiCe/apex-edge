-- Local gift card and loyalty account state.

CREATE TABLE IF NOT EXISTS gift_cards (
    id TEXT PRIMARY KEY NOT NULL,
    code TEXT NOT NULL UNIQUE,
    balance_cents INTEGER NOT NULL DEFAULT 0,
    currency TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS loyalty_accounts (
    customer_id TEXT PRIMARY KEY NOT NULL,
    points INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);
