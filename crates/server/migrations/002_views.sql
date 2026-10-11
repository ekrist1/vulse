CREATE TABLE IF NOT EXISTS collection_views (
    id TEXT PRIMARY KEY,
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    config TEXT NOT NULL,
    UNIQUE(collection_id, name)
);
CREATE TABLE IF NOT EXISTS document_cells (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    field TEXT NOT NULL,
    text_value TEXT NOT NULL,
    raw_value TEXT NOT NULL,
    number_value TEXT,
    PRIMARY KEY(document_id, field)
);
CREATE INDEX IF NOT EXISTS idx_document_cells_values ON document_cells(field, text_value, document_id);
