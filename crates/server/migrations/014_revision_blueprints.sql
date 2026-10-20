-- Each record revision kept a full copy of its collection's blueprint. The
-- blueprints are now stored once, by the SHA-256 of their text, and
-- record_revisions.blueprint holds that id. Existing rows are converted by
-- the migration code.
CREATE TABLE revision_blueprints (id TEXT PRIMARY KEY, blueprint TEXT NOT NULL);
