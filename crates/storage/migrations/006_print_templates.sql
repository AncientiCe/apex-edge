-- Print templates synced from HQ; one active template per (store_id, document_type).
-- template_id is the HQ template id; version used for ordering; we keep latest per store+document_type.
CREATE TABLE IF NOT EXISTS print_templates (
    store_id TEXT NOT NULL,
    document_type TEXT NOT NULL,
    template_id TEXT NOT NULL,
    template_body TEXT NOT NULL,
    version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (store_id, document_type)
);
