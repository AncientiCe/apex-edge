-- Payment provider metadata for terminal-backed tenders.

ALTER TABLE order_payments ADD COLUMN tip_amount_cents INTEGER NOT NULL DEFAULT 0;
ALTER TABLE order_payments ADD COLUMN provider TEXT;
ALTER TABLE order_payments ADD COLUMN provider_payment_id TEXT;
ALTER TABLE order_payments ADD COLUMN entry_method TEXT;

CREATE INDEX IF NOT EXISTS order_payments_provider_idx ON order_payments (provider);
CREATE INDEX IF NOT EXISTS order_payments_provider_payment_id_idx ON order_payments (provider_payment_id);
