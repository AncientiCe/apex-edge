-- Latest sync run (single row, id = 'latest')
CREATE TABLE IF NOT EXISTS sync_run (
    id TEXT PRIMARY KEY NOT NULL DEFAULT 'latest',
    state TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT,
    last_error TEXT
);

-- Per-entity latest sync status (for status page)
CREATE TABLE IF NOT EXISTS entity_sync_status (
    entity TEXT PRIMARY KEY NOT NULL,
    current INTEGER NOT NULL DEFAULT 0,
    total INTEGER,
    percent REAL,
    updated_at TEXT,
    status TEXT NOT NULL
);
