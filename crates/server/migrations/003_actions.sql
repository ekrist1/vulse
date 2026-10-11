CREATE TABLE IF NOT EXISTS record_actions (
    id TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    snapshot TEXT NOT NULL,
    created_at TEXT NOT NULL
);
