-- Shared, transactional change counters: visible only after commit, on every
-- server process. Tombstones remain so deleting a watched collection is noticed.
CREATE TABLE collection_versions (
    collection_id TEXT PRIMARY KEY,
    version INTEGER NOT NULL
);
INSERT INTO collection_versions SELECT id, 1 FROM collections;
CREATE INDEX IF NOT EXISTS idx_documents_collection_id ON documents(collection_id, id);
CREATE TRIGGER collection_version_insert AFTER INSERT ON collections BEGIN
    INSERT INTO collection_versions VALUES (NEW.id, 1)
    ON CONFLICT(collection_id) DO UPDATE SET version=version+1;
END;
CREATE TRIGGER collection_version_update AFTER UPDATE ON collections BEGIN
    INSERT INTO collection_versions VALUES (NEW.id, 1)
    ON CONFLICT(collection_id) DO UPDATE SET version=version+1;
END;
CREATE TRIGGER collection_version_delete AFTER DELETE ON collections BEGIN
    INSERT INTO collection_versions VALUES (OLD.id, 1)
    ON CONFLICT(collection_id) DO UPDATE SET version=version+1;
END;
CREATE TRIGGER document_version_insert AFTER INSERT ON documents BEGIN
    INSERT INTO collection_versions VALUES (NEW.collection_id, 1)
    ON CONFLICT(collection_id) DO UPDATE SET version=version+1;
END;
CREATE TRIGGER document_version_update AFTER UPDATE ON documents BEGIN
    INSERT INTO collection_versions VALUES (NEW.collection_id, 1)
    ON CONFLICT(collection_id) DO UPDATE SET version=version+1;
    UPDATE collection_versions SET version=version+1
    WHERE collection_id=OLD.collection_id AND OLD.collection_id<>NEW.collection_id;
END;
CREATE TRIGGER document_version_delete AFTER DELETE ON documents BEGIN
    INSERT INTO collection_versions VALUES (OLD.collection_id, 1)
    ON CONFLICT(collection_id) DO UPDATE SET version=version+1;
END;
