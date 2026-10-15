-- Record tables of collections whose formulas read other collections could
-- hold values from before those collections last changed. The tables are
-- rebuilt from the record bodies after this migration; writes now keep them
-- current (see records::refresh_readers).
CREATE INDEX IF NOT EXISTS idx_documents_collection_order ON documents(collection_id, created_at, id);
