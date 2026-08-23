-- Hub identity: the store and default register this process serves.
-- One row only (id = 1). Survives restarts when APEX_EDGE_STORE_ID is unset.

CREATE TABLE IF NOT EXISTS hub_identity (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    store_id TEXT NOT NULL,
    register_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
