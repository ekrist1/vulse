CREATE TABLE document_links (
 source_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
 field TEXT NOT NULL, kind TEXT NOT NULL, target_collection TEXT NOT NULL,
 target_id TEXT NOT NULL, PRIMARY KEY(source_id,field,kind,target_collection,target_id)
);
CREATE INDEX links_target ON document_links(kind,target_collection,target_id);
