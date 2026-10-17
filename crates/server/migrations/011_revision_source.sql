-- Where a change came from when it was not the web app or REST API, such as
-- 'mcp' for changes made by an AI assistant.
ALTER TABLE record_revisions ADD COLUMN source TEXT;
