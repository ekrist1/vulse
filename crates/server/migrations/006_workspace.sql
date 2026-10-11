-- One shared workspace: collections are no longer scoped to the user who
-- created them, and accounts carry a role. Accounts that existed before this
-- migration each managed their own collections, so they become admins.
ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'viewer';
ALTER TABLE users ADD COLUMN disabled_at TEXT;
UPDATE users SET role = 'admin';

ALTER TABLE api_keys ADD COLUMN role TEXT NOT NULL DEFAULT 'viewer';
UPDATE api_keys SET role = 'admin';

-- Session cookies are now stored as SHA-256 hashes. Existing plaintext
-- sessions are dropped, so everyone signs in again once.
DROP TABLE sessions;
CREATE TABLE auth_sessions (
    token_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_auth_sessions_user ON auth_sessions(user_id);
CREATE INDEX idx_auth_sessions_expiry ON auth_sessions(expires_at);

-- Single-use invite and password reset links, stored as SHA-256 hashes.
CREATE TABLE account_links (
    token_hash TEXT PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    purpose TEXT NOT NULL CHECK (purpose IN ('invite', 'reset')),
    email TEXT NOT NULL,
    role TEXT NOT NULL,
    user_id TEXT REFERENCES users(id) ON DELETE CASCADE,
    created_by TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_account_links_expiry ON account_links(expires_at);
