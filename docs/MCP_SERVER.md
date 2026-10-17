# MCP server design

Status: proposed · Owner: Espen Kristensen

Vulse will expose an [MCP](https://modelcontextprotocol.io) server so Claude (Claude.ai connectors, Claude Desktop, Claude Code) and other MCP clients can query, insert and update records in collections. Users connect with OAuth 2.1 against Vulse itself. API keys also work, for clients configured with a header.

## Goals

- Read and write records through MCP tools, with the same validation, history, undo, events and calculated fields as the web UI.
- Permissions are identical to the REST API: role (viewer, editor, admin) and optional collection scope.
- Connect from Claude.ai as a custom connector with no manual token handling (OAuth).
- Users can see and revoke connected clients.
- User and admin documentation on how to connect and use it.

## Non-goals (first version)

- Schema changes (create/alter collections), user management, imports and asset uploads over MCP.
- An external identity provider. Vulse already owns users, passwords and sessions.
- MCP resources, prompts and sampling. Tools only.
- The stdio transport. Vulse is a server, so MCP is served over HTTP.

## Architecture

```
Claude ──HTTPS──▶ /mcp  (rmcp StreamableHttpService, nested in the axum router)
                   │  mcp_auth layer: Bearer → Principal (OAuth token or API key)
                   ▼
              tool handlers ──▶ records::run_query, mutations::run_bulk, content::* core fns
                                (same code paths as /api/v1)

Claude ──▶ /.well-known/oauth-protected-resource      (RFC 9728)
       ──▶ /.well-known/oauth-authorization-server    (RFC 8414)
       ──▶ /oauth/register  /oauth/authorize  /oauth/token
                   │
                   └─ /oauth/authorize redirects to the web app's sign-in and consent page
```

Vulse acts as both the **resource server** (`/mcp`) and the **authorization server** (`/oauth/*`). Both live in `vulse-server`. The consent page lives in `vulse-web`.

### Routing

`/mcp`, `/.well-known/*` and `/oauth/*` are mounted at the router root in `lib.rs::app`, **outside** `/api/v1`, so `security::same_origin_writes` does not apply to them:

- `/oauth/token` and `/oauth/register` are called cross-origin by the client's backend, without a bearer header.
- `/mcp` requires a bearer header. Cookies are ignored, so cross-site requests cannot ride on a browser session.

The consent decision (`POST /api/v1/oauth/consent`) **does** stay under `/api/v1`. It is cookie-authenticated and relies on `same_origin_writes` for CSRF protection.

## Authentication model

### Principal

```rust
pub enum AuthVia { Session, ApiKey, OAuth }
```

`authenticate()` resolves a bearer token in this order:

1. API key (existing `api_key_principal`).
2. OAuth access token (new `oauth_principal`): look up `sha256(token)` in `oauth_tokens`, reject if expired or revoked, then build a `Principal`:
   - `role` = the lower of the role granted by the token's scope and the owner's **current** role (same rule as API keys).
   - `collection_scope` = the collection chosen at consent, if any.
   - Disabled users are rejected.

`SessionOnly` and `AdminSession` keep rejecting `AuthVia::OAuth`, so OAuth tokens can never manage users, keys or other OAuth clients.

API keys and OAuth tokens use different prefixes (e.g. `vk_…` and `vat_…` / `vrt_…`). This makes them easy to tell apart in logs and secret scanners and avoids a second lookup.

### Scopes

| Scope | Grants | Maps to |
|---|---|---|
| `records:read` | list collections, describe schemas, query and get records | Viewer |
| `records:write` | create, update and delete records | Editor |

Admin is never granted through OAuth. Schema changes are out of scope.

At consent, the user may optionally limit the grant to **one collection**. This mirrors API key `collection_id` and is stored on the grant.

### Tokens

- Opaque random tokens (`new_token()`), stored as SHA-256 hashes, matching sessions and API keys. No JWTs: the same server issues and checks tokens, and opaque tokens can be revoked immediately.
- Access token lifetime: 1 hour.
- Refresh token lifetime: 30 days, **rotated on every use**. Presenting an already-used refresh token revokes the whole grant (reuse detection).
- Authorization codes: single use, 60 seconds, bound to `client_id`, `redirect_uri`, PKCE challenge and `resource`.

## OAuth endpoints

Follows the MCP authorization spec (OAuth 2.1 profile).

### `GET /.well-known/oauth-protected-resource`

```json
{
  "resource": "https://vulse.example.com/mcp",
  "authorization_servers": ["https://vulse.example.com"],
  "scopes_supported": ["records:read", "records:write"],
  "bearer_methods_supported": ["header"]
}
```

Unauthenticated or invalid requests to `/mcp` return `401` with
`WWW-Authenticate: Bearer resource_metadata="https://vulse.example.com/.well-known/oauth-protected-resource"`.
A token without the write scope reaches `/mcp`, but the write tools refuse it with a tool error ("this action requires the editor role"), because the scope sets the principal's role.

The base URL comes from the configured public origin (`AuthSettings::public_origin`), never from the `Host` header. Make this setting required when MCP is enabled.

### `GET /.well-known/oauth-authorization-server`

Advertises `issuer`, `authorization_endpoint`, `token_endpoint`, `registration_endpoint`, `response_types_supported: ["code"]`, `grant_types_supported: ["authorization_code", "refresh_token"]`, `code_challenge_methods_supported: ["S256"]`, `token_endpoint_auth_methods_supported: ["none"]`, `scopes_supported`.

### `POST /oauth/register` (Dynamic Client Registration, RFC 7591)

- Accepts `client_name`, `redirect_uris`, `grant_types`, `token_endpoint_auth_method` (`none` only: public clients with PKCE).
- `redirect_uris` must be `https://…`, or `http://localhost` / `http://127.0.0.1` with any port (for desktop and CLI clients). Reject fragments.
- Rate limited per address with the existing `Throttle`.
- Admins can close registration (phase 5). New clients then wait for an admin's approval before `/oauth/authorize` accepts them.
- Unused clients (no grant within 24h) are deleted by the cleanup job.

Later: support Client ID Metadata Documents (the `client_id` is an HTTPS URL to a JSON document) alongside DCR.

### `GET /oauth/authorize`

Required params: `response_type=code`, `client_id`, `redirect_uri` (exact match with a registered one), `code_challenge`, `code_challenge_method=S256`, `resource` (must equal the `/mcp` URL), `state`. Optional: `scope` (default `records:read`).

- Invalid `client_id` or `redirect_uri`: show an error page. **Never redirect.**
- Other errors: redirect to `redirect_uri` with `error=…&state=…`.
- Valid: store a pending authorization request (5 min TTL), then redirect to the web app at `/oauth/consent?request=<id>`. The web app sends the user through sign-in first if there is no session.

### Consent page (`vulse-web`, `pages/consent.rs`)

Shows: client name and redirect host, the account being connected, requested permissions in plain language, an optional collection picker, and Allow / Deny buttons.

- Requesting `records:write` as a viewer: show the grant as read-only.
- `POST /api/v1/oauth/consent` (`SessionOnly`) with `{request, approve, collection_id}` returns the redirect URL containing `code` and `state`.

### `POST /oauth/token`

Form-encoded.

- `grant_type=authorization_code`: `code`, `redirect_uri`, `client_id`, `code_verifier`, `resource`. Verify PKCE (`BASE64URL(SHA256(verifier)) == challenge`), single-use code, matching client, redirect and resource.
- `grant_type=refresh_token`: `refresh_token`, `client_id`, optional narrower `scope`. Rotate.
- Response: `{access_token, token_type:"Bearer", expires_in, refresh_token, scope}`, with `Cache-Control: no-store`.
- Errors use RFC 6749 codes (`invalid_grant`, `invalid_client`, `invalid_request`, `unsupported_grant_type`).
- Throttled per client and per address.

### Managing grants

- `GET /api/v1/oauth/grants` (`SessionOnly`): list my connected clients (name, scope, collection, created, last used).
- `DELETE /api/v1/oauth/grants/{id}` (`SessionOnly`): revoke a grant and all its tokens.
- Admins: `GET /api/v1/oauth/grants?all=true` lists every grant with its owner's handle, and admins can revoke any grant. On the Keys page, admins see the workspace-wide list.
- Everything that ends a user's sessions also deletes their grants (`store::delete_user_sessions`): disabling the user, a password change or reset, and "log out everywhere".
- Web: a **Connected apps** section on the Keys page.

## Database (migration `012_oauth.sql`)

Tables: `oauth_clients`, `oauth_requests` (pending consent, 10 min), `oauth_grants` (one per approval), `oauth_codes` (5 min, with `used_at` for reuse detection) and `oauth_tokens` (`kind` is `access` or `refresh`, with `used_at` for refresh rotation). Codes and tokens are stored as SHA-256 hashes. Revoking a grant deletes it and its codes and tokens. `last_used_at` on a grant is updated whenever tokens are issued, so it is accurate to about an hour. See `crates/server/migrations/012_oauth.sql`.

Expired requests, codes and tokens, and clients that nobody approved within a day, are pruned by the hourly cleanup (`oauth::purge`, called from `store::purge_expired`).

## MCP server

### Crate and transport

`rmcp` (`modelcontextprotocol/rust-sdk`) with features `server`, `macros` and `transport-streamable-http-server`. `StreamableHttpService` is a tower service, mounted with `nest_service("/mcp", …)`.

A small `mcp_auth` middleware runs in front of it:

- Resolves the bearer token to a `Principal`, or returns the 401 challenge described above.
- Inserts the `Principal` into request extensions. Tool handlers read it from the rmcp request context (`RequestContext` → `http::request::Parts` extensions).
- Validates `Origin` when present (spec requirement against DNS rebinding).

Sessions: none. The service runs stateless (`legacy_session_mode = false`) with JSON responses. Every request carries its own credential, any instance can answer it, and there is no session to hijack with a different token. `Host` is limited to localhost plus the host of `VULSE_PUBLIC_URL`, and a browser `Origin` must match `VULSE_PUBLIC_URL`.

### Refactor needed first

The REST handlers are axum handlers that take extractors. Pull their bodies into plain functions that take `(&AppState or &Conn, &Principal, input)`, and have both the REST handlers and the MCP tools call them. Candidates:

- `content::list_collections`, `content::get_collection`, `content::get_document`, `content::create_document`
- `records::query_records` (already wraps `run_query`)
- `mutations::patch_document`, `mutations::bulk_records` (already wraps `run_bulk`)
- `sql::run_sql` (only if `run_sql` ships, see below)

Permission checks (`require_role`, `require_collection`) move into these functions or stay right next to them, so neither entry point can skip them.

### Tools

Tool descriptions are written for the model: say what the tool does, when to use it, and the value formats it expects. Outputs are compact JSON. Large results are paginated with an explicit `next_offset`.

| Tool | Scope | Input | Notes |
|---|---|---|---|
| `list_collections` | read | none | id, name, description, record count. Respects collection scope. |
| `describe_collection` | read | `collection` (id or name) | Fields with type, required, options for selects, relationship targets, calculated (read-only) marker. **Call before querying or writing.** |
| `query_records` | read | `collection`, `search?`, `filters?` (`RecordFilter` incl. `or`), `sort?`, `descending?`, `limit` (default 25, max 100), `offset?` | Maps to `RecordQuery` and `run_query`. Returns records, total and `next_offset`. |
| `get_record` | read | `collection`, `id` | Full record including calculated values and `calculation_errors`. |
| `create_records` | write | `collection`, `records: [ {field: value} ]` (max 100) | All or nothing (`mutations::run_create`). Calculated fields in the input are ignored and listed in `ignored_calculated_fields`. Returns the records and an `undo_id`. |
| `update_records` | write | `collection`, `updates: [ {id, fields, expected_updated_at?} ]` (max 100) | Each update has its own fields. All or nothing, with optional version checks (`mutations::run_changes`). Returns the records and an `undo_id`. |
| `delete_records` | write | `collection`, `ids` (max 100) | All or nothing. Deleted records go to the trash and can be restored. Records still referenced are refused. Returns an `undo_id`. |
| `undo_action` | write | `collection`, `undo_id` | Reverts one write from the last 24 hours (`mutations::run_undo`). Refused if an affected record changed since. |
| `run_sql` *(optional, phase 2)* | read | `sql` | `SELECT` only over MCP at first. Lets Claude do ad-hoc aggregation. |

Tool annotations: read tools get `readOnlyHint: true`. Write tools get `readOnlyHint: false`. `delete_records` and `undo_action` (which deletes records when it reverts a create) also get `destructiveHint: true`. This lets clients ask the user to confirm.

Errors: validation problems are returned as tool results with `isError: true` and the `AppError` message, so the model can correct itself. Auth problems are returned as protocol and HTTP errors.

### Server instructions

The `initialize` response includes short instructions, for example: "Vulse stores records in collections. Call `list_collections`, then `describe_collection` before querying or writing. Field names are case-sensitive. Calculated fields are read-only."

### Auditing

Record history does not store who made a change, for any client. MCP writes set `source = 'mcp'` on each revision (migration `011_revision_source.sql`, set through `Conn::set_source`), and the web history list shows "via AI assistant (MCP)". Recording the acting user (and the OAuth client name) on every revision is a separate change for all clients, not only MCP.

## Security checklist

- [ ] PKCE S256 required. `plain` rejected.
- [ ] `resource` checked at authorize, at token exchange, and bound to the token. Tokens are only accepted on `/mcp`. OAuth tokens are rejected on `/api/v1`, except the read-only endpoints, if we decide to allow them later.
- [ ] Exact `redirect_uri` match. No open redirects on client errors.
- [ ] All secrets (codes, tokens, client registration) stored hashed. Compared by hash lookup.
- [ ] Throttling on `/oauth/register`, `/oauth/token` and failed bearer lookups.
- [ ] Role computed from the owner's current role on every request. Disabling a user or resetting their password revokes grants.
- [ ] OAuth tokens never pass `SessionOnly` / `AdminSession`.
- [ ] `Origin` validated on `/mcp`. `Cache-Control: no-store` on token responses.
- [ ] Public origin required and HTTPS enforced (outside localhost) when MCP is enabled.
- [ ] Tool input limits (batch size ≤ 100, query limit ≤ 100) to bound cost and context size.

## Configuration

| Setting | Default | Purpose |
|---|---|---|
| `VULSE_MCP` | `off` | Enables `/mcp`, `/.well-known/*` and `/oauth/*`. |
| `VULSE_PUBLIC_URL` | `http://BIND` | Existing setting. Used as the OAuth `issuer`, with `/mcp` appended as the `resource`. Its host is added to the MCP `Host` allowlist and its origin to the `Origin` allowlist. Must use `https://` when MCP is on, except for localhost. The server logs a warning when MCP is on without it. It must have no path, because the `.well-known` URLs are served at the root. |

## Implementation plan

Each phase is shippable and tested before the next starts.

### Phase 1: MCP read tools with API keys
- [x] Add `rmcp` dependency. Mount `/mcp` behind the `VULSE_MCP` flag.
- [x] `mcp_auth` middleware: API key bearer → `Principal`, 401 otherwise (`mcp::authorize`, `auth::bearer_principal`).
- [x] Refactor the read handlers into core functions (`content::visible_collections`, `content::read_document`; queries already shared `records::run_query`).
- [x] Tools: `list_collections`, `describe_collection`, `query_records`, `get_record`.
- [x] Tests in `tests/mcp.rs`: bearer-only access, foreign `Origin`, collection-scope enforcement, filters with OR, pagination, calculated values, unknown collection/field/record errors.
- [ ] Manual test with MCP Inspector and Claude Code (`claude mcp add --transport http`).

### Phase 2: write tools
- [x] Refactor create, patch and bulk into core functions (`run_changes` with a patch for each record, `run_create`, `run_undo`; `run_bulk` and the REST handlers use them).
- [x] Tools: `create_records`, `update_records`, `delete_records`, `undo_action`. Each checks the Editor role itself.
- [x] `source = 'mcp'` in history.
- [x] Tests: viewer key rejected, atomic batches, calculated fields ignored or rejected, version conflicts, trash, undo of update, delete and create.

### Phase 3: OAuth authorization server
- [x] Migration `012_oauth.sql`. The queries live in `crates/server/src/oauth.rs`.
- [x] Metadata endpoints (with open CORS) and the 401 challenge.
- [x] `/oauth/register`, `/oauth/authorize`, `/oauth/token` (with PKCE, resource binding, `iss` in responses, refresh rotation and code or refresh reuse detection).
- [x] `AuthVia::OAuth`, accepted only on `/mcp` (`auth::bearer_principal`). The REST API keeps refusing OAuth tokens.
- [x] Consent page (`/oauth/consent`) and returning to it after sign-in, in `vulse-web`.
- [x] Connected apps list and disconnect on the Keys page. Admins see and disconnect every app in the workspace.
- [x] Cleanup of expired rows.
- [x] Tests in `tests/oauth.rs` and `oauth::unit`: metadata, full code flow, wrong PKCE verifier and code reuse, refresh rotation and reuse, unknown client or redirect (error page, no redirect), redirected errors, deny, viewer downgrade, read-only and single-collection grants, disconnect, disabling a user.
- [x] Smoke test against a running server with curl: register, authorize, consent API, token, MCP `initialize` and `create_records`.
- [ ] Consent page tested in a browser.
- [ ] End-to-end: add as a custom connector in Claude.ai and in Claude Desktop. This needs a public https deployment.

### Phase 4: documentation
- [x] `docs/MCP.md`: user and admin guide (outline below).
- [x] Link from `README.md`: a `VULSE_MCP` row in the settings table, and an "AI assistants (MCP)" section.
- [x] In-app: the Keys page shows the `/mcp` address with a copy button and a one-line hint for connecting Claude.

### Phase 5: later
- [x] `run_sql` (SELECT only): `sql::select_only`, shared with `POST /sql`. Up to 200 rows go back to the model.
- [x] Client ID Metadata Documents (`client_metadata.rs`). An `https://` `client_id` is read from its URL and cached for a day; the cached copy is used if the site is down. Fetching is limited to public addresses: Vulse resolves the name, refuses loopback, private, link-local, shared and translation ranges, connects only to the address it checked, follows no redirects, and stops at 5 s or 64 KiB. Each fetch counts against the address's rate limit. The document must name its own URL as `client_id` and be a public client. The consent page shows the domain. Advertised as `client_id_metadata_document_supported`.
- [x] Closing registration (workspace setting `mcp_open_registration` in the new `workspace_settings` table, migration `013_oauth_clients.sql`) with admin approval. Clients registered or first seen while it is closed get no `approved_at`, and `/oauth/authorize` shows a page asking for an admin instead. Admins manage it under **API keys → Apps that can connect**: `GET/PUT /api/v1/oauth/settings`, `GET /api/v1/oauth/clients`, `POST /api/v1/oauth/clients/{id}/approve`, `DELETE /api/v1/oauth/clients/{id}`. Clients an admin approved (`approved_by`) are never removed by cleanup; ones still waiting are removed after 7 days. Error pages escape the client's name.
- [ ] ~~MCP resources for collection schemas.~~ Not planned. `describe_collection` already gives the model each schema when it needs it, and Claude reads MCP resources only when a user attaches them by hand, so resources would duplicate the tool without being used. Reconsider if a client starts reading resources automatically.

## User and admin documentation (`docs/MCP.md`)

Written in the same style as `docs/CALCULATED_FIELDS.md`: task-focused and plain. Planned outline:

**For admins**
1. What the MCP server is and what it can and cannot do (records only, no schema or user changes).
2. Enabling it: `VULSE_MCP=on`, `VULSE_PUBLIC_URL`, HTTPS requirement, reverse proxy notes (pass through `Authorization`, don't buffer streaming responses).
3. Controlling access: roles and what each can do via MCP, open vs. closed client registration, approving clients.
4. Auditing and revoking: where MCP changes appear in history, viewing and revoking any user's connected apps, effect of disabling a user.

**For users**
1. Connecting Claude.ai: Settings → Connectors → Add custom connector → `https://<your-vulse>/mcp` → sign in → consent (with a screenshot of the consent page and what each option means).
2. Connecting Claude Desktop (same connector flow).
3. Connecting Claude Code: `claude mcp add --transport http vulse https://<your-vulse>/mcp`, which uses OAuth automatically. Alternatively, pass an API key with `--header "Authorization: Bearer <key>"`.
4. Other MCP clients: server URL, OAuth discovery, or API key header.
5. Limiting access: read-only vs. read and write, limiting to one collection, using a scoped API key.
6. Example prompts: "List my collections", "Find all orders over 1000 from last month", "Mark these 12 tasks as done", "Undo that".
7. Revoking access: Keys page → Connected apps.
8. Troubleshooting: 401 or reconnect loops (expired grant, user disabled), "insufficient scope", collection not visible (collection-scoped grant), proxy stripping headers.

**Reference**
- Tool list with inputs and examples (generated from the tool definitions, or kept in sync by a test that compares the names).
