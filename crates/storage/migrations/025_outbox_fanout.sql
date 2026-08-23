-- Makes the destination fan-out from migration 018 usable.
--
-- 018 created outbox_destinations and outbox_delivery_attempts but nothing read them:
-- delivery was a single hardcoded POST to HQ. Fan-out needs two guarantees the original
-- schema does not give:
--
--   * one delivery row per (submission, destination) and no more, so a dispatcher that
--     restarts mid-cycle cannot deliver the same submission to the same place twice;
--   * a way to find work across all destinations at once, rather than per destination.

CREATE UNIQUE INDEX IF NOT EXISTS outbox_delivery_unique_idx
    ON outbox_delivery_attempts (outbox_id, destination_id);

CREATE INDEX IF NOT EXISTS outbox_delivery_due_idx
    ON outbox_delivery_attempts (status, next_attempt_at);

-- Finding submissions that still owe someone a delivery is the hot path of every cycle.
CREATE INDEX IF NOT EXISTS outbox_status_idx ON outbox (status, created_at);
