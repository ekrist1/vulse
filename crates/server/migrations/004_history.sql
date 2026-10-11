CREATE TABLE record_revisions (
 id TEXT PRIMARY KEY, collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
 document_id TEXT NOT NULL, action TEXT NOT NULL, body TEXT NOT NULL, blueprint TEXT NOT NULL,
 record_created_at TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX revisions_record ON record_revisions(collection_id,document_id,created_at DESC,id DESC);
INSERT INTO record_revisions SELECT lower(hex(randomblob(16))),d.collection_id,d.id,'baseline',d.body,c.blueprint,d.created_at,d.updated_at FROM documents d JOIN collections c ON c.id=d.collection_id;
