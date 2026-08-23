-- Durable record of every provider-side payment, so the hub can always answer
-- "did we take this money, and does the customer still owe it?" after a crash.
--
-- The dangerous window in a store hub is between capturing a card and having a
-- durable order. A captured intent whose cart never finalized is money taken for
-- nothing; the reversal sweeper finds those rows and voids them.
--
-- state:
--   authorized       funds held, not yet taken
--   captured         funds taken, order not yet confirmed durable
--   settled          funds taken and the order is on the ledger
--   reversal_pending capture succeeded but the sale did not; owes a void
--   reversed         void/refund confirmed by the provider
--   declined         provider said no; nothing to reverse
--   failed           provider could not be reached and no funds moved

CREATE TABLE IF NOT EXISTS payment_intents (
    id TEXT PRIMARY KEY NOT NULL,
    store_id TEXT NOT NULL,
    register_id TEXT NOT NULL,
    cart_id TEXT NOT NULL,
    tender_id TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    provider TEXT NOT NULL,
    provider_payment_id TEXT,
    amount_cents INTEGER NOT NULL,
    tip_amount_cents INTEGER NOT NULL DEFAULT 0,
    approved_cents INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL,
    failure_code TEXT,
    order_id TEXT,
    reversal_attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- A retried POS command must resolve to the existing intent, never a second charge.
CREATE UNIQUE INDEX IF NOT EXISTS payment_intents_idem_idx
    ON payment_intents (idempotency_key, provider, tender_id);

CREATE INDEX IF NOT EXISTS payment_intents_cart_idx ON payment_intents (store_id, cart_id, state);
CREATE INDEX IF NOT EXISTS payment_intents_state_idx ON payment_intents (state, updated_at);
