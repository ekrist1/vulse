-- Each collection gets its own typed record table (see rows.rs), built from
-- the record bodies after this migration. It replaces the cell table.
DROP TABLE IF EXISTS document_cells;
-- Pages in creation order, with the id as tie-breaker, read from the index.
CREATE INDEX IF NOT EXISTS idx_documents_collection_order ON documents(collection_id, created_at, id);
