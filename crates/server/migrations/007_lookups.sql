-- Formula columns of other collections are cached until a collection's
-- record count or latest update changes; this keeps that check cheap.
CREATE INDEX IF NOT EXISTS idx_documents_updated ON documents(collection_id, updated_at);
