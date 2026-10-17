-- Workspace-wide settings, as key and value.
CREATE TABLE workspace_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- When the client may start sign-ins. Clients registered while registration
-- is closed wait for an admin. approved_by is set when an admin approved it,
-- which also keeps it from being removed while unused.
ALTER TABLE oauth_clients ADD COLUMN approved_at TEXT;
ALTER TABLE oauth_clients ADD COLUMN approved_by TEXT;
-- For clients identified by a metadata document URL: when it was last read.
ALTER TABLE oauth_clients ADD COLUMN fetched_at TEXT;
UPDATE oauth_clients SET approved_at = created_at;
