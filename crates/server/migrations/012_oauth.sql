-- OAuth 2.1 for MCP clients. Codes and tokens are stored as SHA-256 hashes.
CREATE TABLE oauth_clients (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    redirect_uris TEXT NOT NULL,
    created_at TEXT NOT NULL
);

-- An authorization request waiting for the user's consent.
CREATE TABLE oauth_requests (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    scope TEXT NOT NULL,
    state TEXT,
    code_challenge TEXT NOT NULL,
    resource TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_oauth_requests_expiry ON oauth_requests(expires_at);

-- One approval by one user. Deleting it disconnects the client.
CREATE TABLE oauth_grants (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES oauth_clients(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scope TEXT NOT NULL,
    collection_id TEXT,
    created_at TEXT NOT NULL,
    last_used_at TEXT
);
CREATE INDEX idx_oauth_grants_user ON oauth_grants(user_id);

CREATE TABLE oauth_codes (
    code_hash TEXT PRIMARY KEY,
    grant_id TEXT NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    resource TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used_at TEXT
);
CREATE INDEX idx_oauth_codes_grant ON oauth_codes(grant_id);

CREATE TABLE oauth_tokens (
    token_hash TEXT PRIMARY KEY,
    grant_id TEXT NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used_at TEXT
);
CREATE INDEX idx_oauth_tokens_grant ON oauth_tokens(grant_id);
CREATE INDEX idx_oauth_tokens_expiry ON oauth_tokens(expires_at);
