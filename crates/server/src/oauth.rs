//! OAuth 2.1 authorization server for MCP clients, following the MCP
//! authorization spec: metadata discovery (RFC 9728, RFC 8414), dynamic client
//! registration (RFC 7591), authorization code with PKCE, resource indicators
//! (RFC 8707) and rotating refresh tokens. Clients may also identify
//! themselves with a metadata document URL instead of registering
//! (`client_metadata`). Users approve clients on the web app's consent page
//! while signed in; when an admin closes registration, new clients also need
//! an admin's approval.

use std::collections::HashMap;

use axum::extract::{Path, Query, Request, State};
use axum::http::header::{self, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use vulse_model::{
    OAuthClientView, OAuthConsent, OAuthGrantView, OAuthRedirect, OAuthRequestView, OAuthSettings,
    Role,
};

use crate::auth::{AdminSession, AuthVia, Principal, SessionOnly, hash_secret, new_id, new_token};
use crate::client_metadata;
use crate::db::{Conn, SqlValue, opt_text_at, text_at};
use crate::error::AppError;
use crate::security::{ADDRESS_LIMIT, ClientIp, address_key};
use crate::state::AppState;
use crate::{store, timeutil};

pub const SCOPE_READ: &str = "records:read";
pub const SCOPE_WRITE: &str = "records:write";
const ACCESS_SECONDS: i64 = 60 * 60;
const REFRESH_SECONDS: i64 = 30 * 24 * 60 * 60;
const CODE_SECONDS: i64 = 5 * 60;
const REQUEST_SECONDS: i64 = 10 * 60;
const ACCESS_PREFIX: &str = "vat_";
const REFRESH_PREFIX: &str = "vrt_";
const MAX_REDIRECT_URIS: usize = 10;
/// How long a client's metadata document is used before it is read again.
const METADATA_SECONDS: i64 = 24 * 60 * 60;
const OPEN_REGISTRATION: &str = "mcp_open_registration";

/// Discovery, registration, authorization and token endpoints at the root.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .route("/oauth/register", post(register))
        .route("/oauth/token", post(token))
        .layer(axum::middleware::from_fn(open_cors))
        .route("/oauth/authorize", get(authorize))
}

/// Consent and connected-app endpoints, under `/api/v1` for signed-in users.
pub fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/oauth/requests/{id}", get(request_info))
        .route("/oauth/consent", post(consent))
        .route("/oauth/grants", get(list_grants))
        .route("/oauth/grants/{id}", delete(delete_grant))
        .route("/oauth/settings", get(get_settings).put(put_settings))
        .route("/oauth/clients", get(list_clients))
        .route("/oauth/clients/{id}/approve", post(approve_client))
        .route("/oauth/clients/{id}", delete(delete_client))
}

/// The protected MCP endpoint.
pub fn resource(state: &AppState) -> String {
    format!("{}/mcp", state.auth.base_url)
}

/// `WWW-Authenticate` for requests to `/mcp` without a valid token, pointing
/// clients at the metadata that starts the OAuth flow.
pub fn challenge(state: &AppState) -> HeaderValue {
    let value = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
        state.auth.base_url
    );
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("Bearer"))
}

pub fn is_access_token(token: &str) -> bool {
    token.starts_with(ACCESS_PREFIX)
}

/// The principal behind an access token. Writes need the write scope and an
/// owner who is at least an editor; admin rights are never granted.
pub(crate) async fn access_principal(conn: &Conn, token: &str) -> Result<Principal, AppError> {
    let rows = conn
        .query(
            "SELECT u.id, u.email, u.handle, u.role, g.scope, g.collection_id, t.expires_at
             FROM oauth_tokens t
             JOIN oauth_grants g ON g.id = t.grant_id
             JOIN users u ON u.id = g.user_id
             WHERE t.token_hash = ?1 AND t.kind = 'access' AND u.disabled_at IS NULL",
            &[text(&hash_secret(token))],
        )
        .await?;
    let row = rows.first().ok_or(AppError::Unauthorized)?;
    if timeutil::is_expired(&text_at(row, 6)?) {
        return Err(AppError::Unauthorized);
    }
    let user_role = Role::parse(&text_at(row, 3)?).unwrap_or(Role::Viewer);
    let granted = if scope_has_write(&text_at(row, 4)?) {
        Role::Editor
    } else {
        Role::Viewer
    };
    Ok(Principal {
        user_id: text_at(row, 0)?,
        email: text_at(row, 1)?,
        handle: text_at(row, 2)?,
        role: granted.min(user_role),
        via: AuthVia::OAuth,
        collection_scope: opt_text_at(row, 5)?,
        session_hash: None,
    })
}

/// Disconnects every app a user approved.
pub(crate) async fn revoke_user(conn: &Conn, user_id: &str) -> anyhow::Result<()> {
    let user = [text(user_id)];
    for sql in [
        "DELETE FROM oauth_tokens WHERE grant_id IN (SELECT id FROM oauth_grants WHERE user_id = ?1)",
        "DELETE FROM oauth_codes WHERE grant_id IN (SELECT id FROM oauth_grants WHERE user_id = ?1)",
        "DELETE FROM oauth_grants WHERE user_id = ?1",
    ] {
        conn.execute(sql, &user).await?;
    }
    Ok(())
}

async fn delete_grant_rows(conn: &Conn, grant_id: &str) -> anyhow::Result<()> {
    let grant = [text(grant_id)];
    for sql in [
        "DELETE FROM oauth_tokens WHERE grant_id = ?1",
        "DELETE FROM oauth_codes WHERE grant_id = ?1",
        "DELETE FROM oauth_grants WHERE id = ?1",
    ] {
        conn.execute(sql, &grant).await?;
    }
    Ok(())
}

/// Removes expired requests, codes and tokens, and clients registered more
/// than a day ago that nobody approved.
pub(crate) async fn purge(conn: &Conn, now: &str) -> anyhow::Result<u64> {
    let mut removed = 0;
    for sql in [
        "DELETE FROM oauth_requests WHERE expires_at < ?1",
        "DELETE FROM oauth_codes WHERE expires_at < ?1",
        "DELETE FROM oauth_tokens WHERE expires_at < ?1",
    ] {
        removed += conn.execute(sql, &[text(now)]).await?;
    }
    let ago = |days| {
        (chrono::Utc::now() - chrono::Duration::days(days))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };
    // Unused clients go after a day, and ones waiting for an admin after a
    // week. Clients an admin approved stay.
    removed += conn
        .execute(
            "DELETE FROM oauth_clients
             WHERE approved_by IS NULL
             AND ((approved_at IS NOT NULL AND created_at < ?1)
                  OR (approved_at IS NULL AND created_at < ?2))
             AND NOT EXISTS (SELECT 1 FROM oauth_grants g WHERE g.client_id = oauth_clients.id)
             AND NOT EXISTS (SELECT 1 FROM oauth_requests r WHERE r.client_id = oauth_clients.id)",
            &[text(&ago(1)), text(&ago(7))],
        )
        .await?;
    Ok(removed)
}

/// Discovery, registration and token requests come from other origins,
/// including browser-based clients. None of them use cookies.
async fn open_cors(request: Request, next: Next) -> Response {
    let preflight = request.method() == Method::OPTIONS;
    let mut response = if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if preflight {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type, authorization, mcp-protocol-version"),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("86400"),
        );
    }
    response
}

async fn protected_resource(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "resource": resource(&state),
        "authorization_servers": [state.auth.base_url],
        "scopes_supported": [SCOPE_READ, SCOPE_WRITE],
        "bearer_methods_supported": ["header"],
        "resource_name": "Vulse",
    }))
}

async fn authorization_server(State(state): State<AppState>) -> Json<Value> {
    let base = &state.auth.base_url;
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "scopes_supported": [SCOPE_READ, SCOPE_WRITE],
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
        "authorization_response_iss_parameter_supported": true,
        "client_id_metadata_document_supported": true,
    }))
}

#[derive(Deserialize)]
struct Registration {
    #[serde(default)]
    redirect_uris: Vec<String>,
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    grant_types: Option<Vec<String>>,
    #[serde(default)]
    response_types: Option<Vec<String>>,
}

/// Dynamic client registration. Every client is public and proves itself with
/// PKCE, so no secret is issued.
async fn register(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<Registration>,
) -> Result<Response, AppError> {
    let address = address_key(&ip);
    state.throttle.check(&[(&address, ADDRESS_LIMIT)])?;
    // Every registration counts, so one address cannot fill the table.
    state.throttle.fail(&[&address]);
    if let Err((code, description)) = check_client_metadata(
        &body.redirect_uris,
        body.grant_types.as_deref(),
        body.response_types.as_deref(),
    ) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": code, "error_description": description})),
        )
            .into_response());
    }
    let name = body
        .client_name
        .map(|name| name.trim().chars().take(100).collect::<String>())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "MCP client".into());
    let id = new_id();
    let now = timeutil::now_secs();
    let conn = state.db.connect().await?;
    let approved_at = if open_registration(&conn).await? {
        text(&now)
    } else {
        SqlValue::Null
    };
    conn.execute(
        "INSERT INTO oauth_clients (id, name, redirect_uris, created_at, approved_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        &[
            text(&id),
            text(&name),
            text(&serde_json::to_string(&body.redirect_uris).map_err(anyhow::Error::from)?),
            text(&now),
            approved_at,
        ],
    )
    .await?;
    let issued_at = chrono::Utc::now().timestamp();
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "client_id": id,
            "client_id_issued_at": issued_at,
            "client_name": name,
            "redirect_uris": body.redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })),
    )
        .into_response())
}

/// Redirect URIs and grant types a client may use, as `(error, description)`
/// for registration when they are not allowed.
pub(crate) fn check_client_metadata(
    redirect_uris: &[String],
    grant_types: Option<&[String]>,
    response_types: Option<&[String]>,
) -> Result<(), (&'static str, &'static str)> {
    if redirect_uris.is_empty()
        || redirect_uris.len() > MAX_REDIRECT_URIS
        || !redirect_uris.iter().all(|uri| valid_redirect(uri))
    {
        return Err((
            "invalid_redirect_uri",
            "redirect_uris must list 1 to 10 https, loopback http or app-specific URIs without fragments",
        ));
    }
    if grant_types.is_some_and(|types| {
        types
            .iter()
            .any(|kind| kind != "authorization_code" && kind != "refresh_token")
    }) || response_types.is_some_and(|types| types.iter().any(|kind| kind != "code"))
    {
        return Err((
            "invalid_client_metadata",
            "only the authorization_code and refresh_token grants are supported",
        ));
    }
    Ok(())
}

struct Client {
    name: String,
    redirect_uris: Vec<String>,
    approved: bool,
}

struct StoredClient {
    client: Client,
    fetched_at: Option<String>,
}

async fn stored_client(conn: &Conn, id: &str) -> Result<Option<StoredClient>, AppError> {
    let rows = conn
        .query(
            "SELECT name, redirect_uris, approved_at, fetched_at FROM oauth_clients WHERE id = ?1",
            &[text(id)],
        )
        .await?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    Ok(Some(StoredClient {
        client: Client {
            name: text_at(row, 0)?,
            redirect_uris: serde_json::from_str(&text_at(row, 1)?).map_err(anyhow::Error::from)?,
            approved: opt_text_at(row, 2)?.is_some(),
        },
        fetched_at: opt_text_at(row, 3)?,
    }))
}

/// A registered client, or one described by the metadata document at its
/// id, read again once a day. `Ok(Err(message))` explains to the user why
/// the client cannot be used.
async fn find_client(
    state: &AppState,
    conn: &Conn,
    id: &str,
    address: &str,
) -> Result<Result<Client, String>, AppError> {
    let stored = stored_client(conn, id).await?;
    if !client_metadata::is_metadata_url(id) {
        return Ok(stored.map(|stored| stored.client).ok_or_else(|| {
            "This app is not registered with Vulse. Start connecting again from the app.".into()
        }));
    }
    let fresh = stored.as_ref().is_some_and(|stored| {
        stored.fetched_at.as_deref().is_some_and(|fetched| {
            chrono::DateTime::parse_from_rfc3339(fetched).is_ok_and(|fetched| {
                chrono::Utc::now()
                    .signed_duration_since(fetched)
                    .num_seconds()
                    < METADATA_SECONDS
            })
        })
    });
    if fresh {
        return Ok(Ok(stored.expect("checked above").client));
    }
    // Each read reaches out to another server, so it counts against the
    // address's limit.
    state.throttle.check(&[(address, ADDRESS_LIMIT)])?;
    state.throttle.fail(&[address]);
    let metadata = match client_metadata::fetch(id).await {
        Ok(body) => client_metadata::parse(id, &body),
        Err(err) => Err(err),
    };
    let metadata = match (metadata, stored) {
        (Ok(metadata), _) => metadata,
        // An outage at the client's site does not disconnect people.
        (Err(err), Some(stored)) => {
            tracing::warn!(client_id = id, error = %err, "using the cached client metadata");
            return Ok(Ok(stored.client));
        }
        (Err(err), None) => return Ok(Err(format!("Vulse cannot use this app: {err}."))),
    };
    let now = timeutil::now_secs();
    let uris = serde_json::to_string(&metadata.redirect_uris).map_err(anyhow::Error::from)?;
    let approved_at = if open_registration(conn).await? {
        text(&now)
    } else {
        SqlValue::Null
    };
    conn.execute(
        "INSERT INTO oauth_clients (id, name, redirect_uris, created_at, approved_at, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?4)
         ON CONFLICT (id) DO UPDATE SET name = ?2, redirect_uris = ?3, fetched_at = ?4",
        &[
            text(id),
            text(&metadata.name),
            text(&uris),
            text(&now),
            approved_at,
        ],
    )
    .await?;
    Ok(stored_client(conn, id)
        .await?
        .map(|stored| stored.client)
        .ok_or_else(|| "This app could not be saved.".to_string()))
}

/// Whether clients may connect without an admin's approval. Open by default.
pub(crate) async fn open_registration(conn: &Conn) -> Result<bool, AppError> {
    let rows = conn
        .query(
            "SELECT value FROM workspace_settings WHERE key = ?1",
            &[text(OPEN_REGISTRATION)],
        )
        .await?;
    Ok(rows
        .first()
        .map(|row| text_at(row, 0))
        .transpose()?
        .is_none_or(|value| value == "true"))
}

/// Starts a sign-in. Problems with the client or its redirect URI are shown
/// here, never sent to an unverified address; other problems go back to the
/// client. A valid request continues on the web app's consent page.
async fn authorize(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, AppError> {
    let param = |name: &str| {
        params
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    };
    let conn = state.db.connect().await?;
    let Some(client_id) = param("client_id") else {
        return Ok(error_page("The request does not name a client."));
    };
    let client = match find_client(&state, &conn, client_id, &address_key(&ip)).await? {
        Ok(client) => client,
        Err(message) => return Ok(error_page(&message)),
    };
    if !client.approved {
        return Ok(error_page(&format!(
            "A workspace admin must approve “{}” before it can connect. Ask an admin to approve it under API keys, then connect again.",
            client.name
        )));
    }
    let redirect_uri = match param("redirect_uri") {
        Some(uri)
            if client
                .redirect_uris
                .iter()
                .any(|known| redirect_matches(known, uri)) =>
        {
            uri.to_string()
        }
        None if client.redirect_uris.len() == 1 => client.redirect_uris[0].clone(),
        _ => {
            return Ok(error_page(
                "The app asked to return to an address it did not register.",
            ));
        }
    };
    let state_param = param("state");
    let fail = |code: &str, description: &str| {
        let mut pairs = vec![
            ("error", code),
            ("error_description", description),
            ("iss", state.auth.base_url.as_str()),
        ];
        if let Some(value) = state_param {
            pairs.push(("state", value));
        }
        Ok(Redirect::to(&with_params(&redirect_uri, &pairs)).into_response())
    };
    if param("response_type") != Some("code") {
        return fail("unsupported_response_type", "response_type must be code");
    }
    let challenge = param("code_challenge").unwrap_or_default();
    if param("code_challenge_method") != Some("S256") || !pkce_value(challenge) {
        return fail(
            "invalid_request",
            "a code_challenge with code_challenge_method S256 is required",
        );
    }
    let resource = resource(&state);
    if param("resource").is_some_and(|value| value.trim_end_matches('/') != resource) {
        return fail("invalid_target", "unknown resource");
    }
    let Some(scope) = parse_scope(param("scope")) else {
        return fail(
            "invalid_scope",
            "supported scopes are records:read and records:write",
        );
    };
    let id = new_token();
    conn.execute(
        "INSERT INTO oauth_requests
         (id, client_id, redirect_uri, scope, state, code_challenge, resource, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        &[
            text(&id),
            text(client_id),
            text(&redirect_uri),
            text(&scope),
            state_param.map_or(SqlValue::Null, text),
            text(challenge),
            text(&resource),
            text(&timeutil::expiry_in(REQUEST_SECONDS)),
        ],
    )
    .await?;
    Ok(Redirect::to(&format!("/oauth/consent?request={id}")).into_response())
}

struct PendingRequest {
    client_id: String,
    client_name: String,
    redirect_uri: String,
    scope: String,
    state: Option<String>,
    code_challenge: String,
    resource: String,
}

async fn find_request(conn: &Conn, id: &str) -> Result<PendingRequest, AppError> {
    let rows = conn
        .query(
            "SELECT r.client_id, c.name, r.redirect_uri, r.scope, r.state, r.code_challenge,
                    r.resource, r.expires_at
             FROM oauth_requests r JOIN oauth_clients c ON c.id = r.client_id
             WHERE r.id = ?1",
            &[text(id)],
        )
        .await?;
    let expired =
        || AppError::bad("This sign-in request has expired. Start connecting again from the app.");
    let row = rows.first().ok_or_else(expired)?;
    if timeutil::is_expired(&text_at(row, 7)?) {
        return Err(expired());
    }
    Ok(PendingRequest {
        client_id: text_at(row, 0)?,
        client_name: text_at(row, 1)?,
        redirect_uri: text_at(row, 2)?,
        scope: text_at(row, 3)?,
        state: opt_text_at(row, 4)?,
        code_challenge: text_at(row, 5)?,
        resource: text_at(row, 6)?,
    })
}

async fn request_info(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
    Path(id): Path<String>,
) -> Result<Json<OAuthRequestView>, AppError> {
    let conn = state.db.connect().await?;
    let request = find_request(&conn, &id).await?;
    Ok(Json(OAuthRequestView {
        client_domain: client_metadata::domain(&request.client_id),
        client_name: request.client_name,
        redirect_host: redirect_host(&request.redirect_uri),
        write_requested: scope_has_write(&request.scope),
        write_allowed: principal.role >= Role::Editor,
    }))
}

/// Records the user's decision and returns where the browser goes next: back
/// to the client with a code, or with `access_denied`.
async fn consent(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
    Json(body): Json<OAuthConsent>,
) -> Result<Json<OAuthRedirect>, AppError> {
    let conn = state.db.connect().await?;
    let issuer = state.auth.base_url.as_str();
    let redirect = conn
        .with_app_tx(async |conn| {
            let request = find_request(conn, &body.request).await?;
            conn.execute(
                "DELETE FROM oauth_requests WHERE id = ?1",
                &[text(&body.request)],
            )
            .await?;
            let mut pairs = vec![("iss", issuer.to_string())];
            if let Some(value) = &request.state {
                pairs.push(("state", value.clone()));
            }
            if !body.approve {
                pairs.push(("error", "access_denied".into()));
                pairs.push(("error_description", "the user declined".into()));
                return Ok(with_owned_params(&request.redirect_uri, &pairs));
            }
            if let Some(collection) = &body.collection_id
                && store::find_collection(conn, collection).await?.is_none()
            {
                return Err(AppError::bad("that collection no longer exists"));
            }
            let write =
                body.write && scope_has_write(&request.scope) && principal.role >= Role::Editor;
            let scope = if write {
                format!("{SCOPE_READ} {SCOPE_WRITE}")
            } else {
                SCOPE_READ.to_string()
            };
            let grant_id = new_id();
            conn.execute(
                "INSERT INTO oauth_grants (id, client_id, user_id, scope, collection_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                &[
                    text(&grant_id),
                    text(&request.client_id),
                    text(&principal.user_id),
                    text(&scope),
                    body.collection_id.as_deref().map_or(SqlValue::Null, text),
                    text(&timeutil::now_secs()),
                ],
            )
            .await?;
            let code = new_token();
            conn.execute(
                "INSERT INTO oauth_codes
                 (code_hash, grant_id, redirect_uri, code_challenge, resource, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                &[
                    text(&hash_secret(&code)),
                    text(&grant_id),
                    text(&request.redirect_uri),
                    text(&request.code_challenge),
                    text(&request.resource),
                    text(&timeutil::expiry_in(CODE_SECONDS)),
                ],
            )
            .await?;
            pairs.push(("code", code));
            Ok(with_owned_params(&request.redirect_uri, &pairs))
        })
        .await?;
    Ok(Json(OAuthRedirect { redirect }))
}

/// The token endpoint: exchanges a code, or rotates a refresh token.
async fn token(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Form(params): Form<HashMap<String, String>>,
) -> Response {
    let address = address_key(&ip);
    if let Err(err) = state.throttle.check(&[(&address, ADDRESS_LIMIT)]) {
        return err.into_response();
    }
    let result = match params.get("grant_type").map(String::as_str) {
        Some("authorization_code") => exchange_code(&state, &params).await,
        Some("refresh_token") => refresh(&state, &params).await,
        _ => Err(TokenError::new(
            "unsupported_grant_type",
            "grant_type must be authorization_code or refresh_token",
        )),
    };
    match result {
        Ok(body) => (
            [
                (header::CACHE_CONTROL, "no-store"),
                (header::PRAGMA, "no-cache"),
            ],
            Json(body),
        )
            .into_response(),
        Err(TokenError::App(err)) => err.into_response(),
        Err(TokenError::OAuth { code, description }) => {
            if code == "invalid_grant" {
                state.throttle.fail(&[&address]);
            }
            (
                StatusCode::BAD_REQUEST,
                [(header::CACHE_CONTROL, "no-store")],
                Json(json!({"error": code, "error_description": description})),
            )
                .into_response()
        }
    }
}

enum TokenError {
    OAuth {
        code: &'static str,
        description: &'static str,
    },
    App(AppError),
}

impl TokenError {
    fn new(code: &'static str, description: &'static str) -> Self {
        Self::OAuth { code, description }
    }

    fn grant(description: &'static str) -> Self {
        Self::new("invalid_grant", description)
    }
}

impl From<AppError> for TokenError {
    fn from(err: AppError) -> Self {
        Self::App(err)
    }
}

impl From<anyhow::Error> for TokenError {
    fn from(err: anyhow::Error) -> Self {
        Self::App(err.into())
    }
}

async fn exchange_code(
    state: &AppState,
    params: &HashMap<String, String>,
) -> Result<Value, TokenError> {
    let param = |name: &str| params.get(name).map(String::as_str).unwrap_or_default();
    let verifier = param("code_verifier");
    if param("code").is_empty() || param("client_id").is_empty() || !pkce_value(verifier) {
        return Err(TokenError::new(
            "invalid_request",
            "code, client_id and a code_verifier are required",
        ));
    }
    let conn = state.db.connect().await?;
    conn.with_app_tx(async |conn| {
        let rows = conn
            .query(
                "SELECT k.grant_id, k.redirect_uri, k.code_challenge, k.resource, k.expires_at,
                            k.used_at, g.client_id, g.scope
                     FROM oauth_codes k JOIN oauth_grants g ON g.id = k.grant_id
                     WHERE k.code_hash = ?1",
                &[text(&hash_secret(param("code")))],
            )
            .await?;
        let Some(row) = rows.first() else {
            return Ok(Err(TokenError::grant("unknown or expired code")));
        };
        let grant_id = text_at(row, 0)?;
        // A code presented twice may have been stolen: end the grant.
        if opt_text_at(row, 5)?.is_some() {
            delete_grant_rows(conn, &grant_id).await?;
            return Ok(Err(TokenError::grant("this code was already used")));
        }
        conn.execute(
            "UPDATE oauth_codes SET used_at = ?1 WHERE code_hash = ?2",
            &[
                text(&timeutil::now_secs()),
                text(&hash_secret(param("code"))),
            ],
        )
        .await?;
        if timeutil::is_expired(&text_at(row, 4)?) {
            return Ok(Err(TokenError::grant("unknown or expired code")));
        }
        if text_at(row, 6)? != param("client_id") {
            return Ok(Err(TokenError::grant(
                "the code was issued to another client",
            )));
        }
        let redirect = param("redirect_uri");
        if !redirect.is_empty() && redirect != text_at(row, 1)? {
            return Ok(Err(TokenError::grant("redirect_uri does not match")));
        }
        let resource = param("resource");
        if !resource.is_empty() && resource.trim_end_matches('/') != text_at(row, 3)? {
            return Ok(Err(TokenError::new("invalid_target", "unknown resource")));
        }
        if pkce_challenge(verifier) != text_at(row, 2)? {
            return Ok(Err(TokenError::grant("code_verifier does not match")));
        }
        Ok(Ok(issue_tokens(conn, &grant_id, &text_at(row, 7)?).await?))
    })
    .await?
}

async fn refresh(state: &AppState, params: &HashMap<String, String>) -> Result<Value, TokenError> {
    let param = |name: &str| params.get(name).map(String::as_str).unwrap_or_default();
    let presented = param("refresh_token");
    if presented.is_empty() || param("client_id").is_empty() {
        return Err(TokenError::new(
            "invalid_request",
            "refresh_token and client_id are required",
        ));
    }
    let conn = state.db.connect().await?;
    conn.with_app_tx(async |conn| {
        let rows = conn
            .query(
                "SELECT t.grant_id, t.expires_at, t.used_at, g.client_id, g.scope
                     FROM oauth_tokens t
                     JOIN oauth_grants g ON g.id = t.grant_id
                     JOIN users u ON u.id = g.user_id
                     WHERE t.token_hash = ?1 AND t.kind = 'refresh' AND u.disabled_at IS NULL",
                &[text(&hash_secret(presented))],
            )
            .await?;
        let Some(row) = rows.first() else {
            return Ok(Err(TokenError::grant("unknown or expired refresh token")));
        };
        let grant_id = text_at(row, 0)?;
        // Refresh tokens are single use. A reused one may have been
        // stolen, so the whole grant ends.
        if opt_text_at(row, 2)?.is_some() {
            delete_grant_rows(conn, &grant_id).await?;
            return Ok(Err(TokenError::grant(
                "this refresh token was already used",
            )));
        }
        if timeutil::is_expired(&text_at(row, 1)?) {
            return Ok(Err(TokenError::grant("unknown or expired refresh token")));
        }
        if text_at(row, 3)? != param("client_id") {
            return Ok(Err(TokenError::grant(
                "the refresh token was issued to another client",
            )));
        }
        let scope = text_at(row, 4)?;
        if let Some(requested) = params.get("scope")
            && !requested
                .split_whitespace()
                .all(|item| scope.split(' ').any(|granted| granted == item))
        {
            return Ok(Err(TokenError::new(
                "invalid_scope",
                "the requested scope exceeds the grant",
            )));
        }
        conn.execute(
            "UPDATE oauth_tokens SET used_at = ?1 WHERE token_hash = ?2",
            &[text(&timeutil::now_secs()), text(&hash_secret(presented))],
        )
        .await?;
        Ok(Ok(issue_tokens(conn, &grant_id, &scope).await?))
    })
    .await?
}

async fn issue_tokens(conn: &Conn, grant_id: &str, scope: &str) -> Result<Value, AppError> {
    let access = format!("{ACCESS_PREFIX}{}", new_token());
    let refresh = format!("{REFRESH_PREFIX}{}", new_token());
    for (token, kind, seconds) in [
        (&access, "access", ACCESS_SECONDS),
        (&refresh, "refresh", REFRESH_SECONDS),
    ] {
        conn.execute(
            "INSERT INTO oauth_tokens (token_hash, grant_id, kind, expires_at) VALUES (?1, ?2, ?3, ?4)",
            &[
                text(&hash_secret(token)),
                text(grant_id),
                text(kind),
                text(&timeutil::expiry_in(seconds)),
            ],
        )
        .await?;
    }
    conn.execute(
        "UPDATE oauth_grants SET last_used_at = ?1 WHERE id = ?2",
        &[text(&timeutil::now_secs()), text(grant_id)],
    )
    .await?;
    Ok(json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": ACCESS_SECONDS,
        "refresh_token": refresh,
        "scope": scope,
    }))
}

#[derive(Deserialize)]
struct GrantsQuery {
    #[serde(default)]
    all: bool,
}

/// The apps the user connected, or with `all=true` every connected app in the
/// workspace, for admins.
async fn list_grants(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
    Query(query): Query<GrantsQuery>,
) -> Result<Json<Vec<OAuthGrantView>>, AppError> {
    if query.all {
        crate::auth::require_role(&principal, Role::Admin)?;
    }
    let conn = state.db.connect().await?;
    let rows = conn
        .query(
            "SELECT g.id, c.name, g.scope, g.collection_id, g.created_at, g.last_used_at, u.handle
             FROM oauth_grants g
             JOIN oauth_clients c ON c.id = g.client_id
             JOIN users u ON u.id = g.user_id
             WHERE ?1 IS NULL OR g.user_id = ?1
             ORDER BY g.created_at DESC, g.id DESC",
            &[if query.all {
                SqlValue::Null
            } else {
                text(&principal.user_id)
            }],
        )
        .await?;
    let grants = rows
        .iter()
        .map(|row| {
            Ok(OAuthGrantView {
                id: text_at(row, 0)?,
                client_name: text_at(row, 1)?,
                write: scope_has_write(&text_at(row, 2)?),
                collection_id: opt_text_at(row, 3)?,
                created_at: text_at(row, 4)?,
                last_used_at: opt_text_at(row, 5)?,
                handle: query.all.then(|| text_at(row, 6)).transpose()?,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Json(grants))
}

/// Disconnects an app. Admins can disconnect anyone's.
async fn delete_grant(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let conn = state.db.connect().await?;
    let rows = conn
        .query(
            "SELECT user_id FROM oauth_grants WHERE id = ?1",
            &[text(&id)],
        )
        .await?;
    let owner = rows.first().map(|row| text_at(row, 0)).transpose()?;
    match owner {
        Some(owner) if owner == principal.user_id || principal.role == Role::Admin => {
            conn.with_app_tx(async |conn| Ok(delete_grant_rows(conn, &id).await?))
                .await?;
            Ok(StatusCode::NO_CONTENT)
        }
        _ => Err(AppError::NotFound),
    }
}

async fn get_settings(
    State(state): State<AppState>,
    AdminSession(_admin): AdminSession,
) -> Result<Json<OAuthSettings>, AppError> {
    let conn = state.db.connect().await?;
    Ok(Json(OAuthSettings {
        open_registration: open_registration(&conn).await?,
    }))
}

/// Opens or closes registration. Clients already approved stay approved.
async fn put_settings(
    State(state): State<AppState>,
    AdminSession(_admin): AdminSession,
    Json(body): Json<OAuthSettings>,
) -> Result<Json<OAuthSettings>, AppError> {
    let conn = state.db.connect().await?;
    conn.execute(
        "INSERT INTO workspace_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = ?2",
        &[
            text(OPEN_REGISTRATION),
            text(if body.open_registration {
                "true"
            } else {
                "false"
            }),
        ],
    )
    .await?;
    Ok(Json(body))
}

/// Every client Vulse knows, those waiting for approval first.
async fn list_clients(
    State(state): State<AppState>,
    AdminSession(_admin): AdminSession,
) -> Result<Json<Vec<OAuthClientView>>, AppError> {
    let conn = state.db.connect().await?;
    let rows = conn
        .query(
            "SELECT c.id, c.name, c.redirect_uris, c.approved_at, c.created_at,
                    (SELECT COUNT(*) FROM oauth_grants g WHERE g.client_id = c.id)
             FROM oauth_clients c
             ORDER BY c.approved_at IS NOT NULL, c.created_at DESC, c.id",
            &[],
        )
        .await?;
    let clients = rows
        .iter()
        .map(|row| {
            let id = text_at(row, 0)?;
            let uris: Vec<String> = serde_json::from_str(&text_at(row, 2)?)?;
            let mut redirect_hosts: Vec<String> =
                uris.iter().map(|uri| redirect_host(uri)).collect();
            redirect_hosts.dedup();
            Ok(OAuthClientView {
                metadata_document: client_metadata::is_metadata_url(&id),
                id,
                name: text_at(row, 1)?,
                redirect_hosts,
                approved: opt_text_at(row, 3)?.is_some(),
                created_at: text_at(row, 4)?,
                connections: crate::db::int_at(row, 5)? as u64,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Json(clients))
}

/// Lets a client connect while registration is closed, and keeps it from
/// being removed while unused.
async fn approve_client(
    State(state): State<AppState>,
    AdminSession(admin): AdminSession,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let conn = state.db.connect().await?;
    let changed = conn
        .execute(
            "UPDATE oauth_clients SET approved_at = COALESCE(approved_at, ?1), approved_by = ?2
             WHERE id = ?3",
            &[text(&timeutil::now_secs()), text(&admin.user_id), text(&id)],
        )
        .await?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Removes a client and disconnects everyone using it. A client that
/// registers itself again, or still has its metadata document, can ask to
/// connect again; while registration is closed it then needs approval.
async fn delete_client(
    State(state): State<AppState>,
    AdminSession(_admin): AdminSession,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let conn = state.db.connect().await?;
    let found = conn
        .with_app_tx(async |conn| {
            let client = [text(&id)];
            for sql in [
                "DELETE FROM oauth_tokens WHERE grant_id IN (SELECT id FROM oauth_grants WHERE client_id = ?1)",
                "DELETE FROM oauth_codes WHERE grant_id IN (SELECT id FROM oauth_grants WHERE client_id = ?1)",
                "DELETE FROM oauth_grants WHERE client_id = ?1",
                "DELETE FROM oauth_requests WHERE client_id = ?1",
            ] {
                conn.execute(sql, &client).await?;
            }
            Ok(conn
                .execute("DELETE FROM oauth_clients WHERE id = ?1", &client)
                .await?
                > 0)
        })
        .await?;
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

fn text(value: &str) -> SqlValue {
    SqlValue::Text(value.into())
}

/// Space-separated scopes, normalized. Asking for nothing asks for both;
/// write implies read.
fn parse_scope(scope: Option<&str>) -> Option<String> {
    let mut write = scope.is_none();
    for item in scope.unwrap_or_default().split_whitespace() {
        match item {
            SCOPE_READ => {}
            SCOPE_WRITE => write = true,
            _ => return None,
        }
    }
    Some(if write {
        format!("{SCOPE_READ} {SCOPE_WRITE}")
    } else {
        SCOPE_READ.to_string()
    })
}

fn scope_has_write(scope: &str) -> bool {
    scope.split(' ').any(|item| item == SCOPE_WRITE)
}

/// A PKCE verifier or challenge: 43 to 128 unreserved characters.
fn pkce_value(value: &str) -> bool {
    (43..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The scheme and the authority's host, port and remainder of a URI.
pub(crate) fn split_uri(uri: &str) -> Option<(String, &str, Option<&str>, &str)> {
    let (scheme, rest) = uri.split_once(':')?;
    let rest = rest.strip_prefix("//")?;
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    Some((scheme.to_ascii_lowercase(), host, port, path))
}

fn is_loopback(host: &str) -> bool {
    ["localhost", "127.0.0.1", "::1"].contains(&host.to_ascii_lowercase().as_str())
}

/// Allowed redirect targets: https, http on this machine, and app-specific
/// schemes for native apps (RFC 8252). Never a fragment.
pub(crate) fn valid_redirect(uri: &str) -> bool {
    if uri.len() > 2000
        || uri.contains('#')
        || uri.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    if !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return false;
    }
    match scheme.as_str() {
        "https" => split_uri(uri).is_some_and(|(_, host, _, _)| !host.is_empty()),
        "http" => split_uri(uri).is_some_and(|(_, host, _, _)| is_loopback(host)),
        "javascript" | "data" | "file" | "vbscript" | "about" | "blob" | "ws" | "wss" => false,
        _ => true,
    }
}

/// Exact match, except that loopback redirects may use any port, since
/// native apps pick a free one at sign-in (RFC 8252).
fn redirect_matches(registered: &str, presented: &str) -> bool {
    if registered == presented {
        return true;
    }
    match (split_uri(registered), split_uri(presented)) {
        (Some((scheme, host, _, path)), Some((other_scheme, other_host, _, other_path))) => {
            scheme == "http"
                && other_scheme == "http"
                && is_loopback(host)
                && host == other_host
                && path == other_path
        }
        _ => false,
    }
}

fn redirect_host(uri: &str) -> String {
    match split_uri(uri) {
        Some((_, host, _, _)) if !host.is_empty() => host.to_string(),
        _ => uri.split(':').next().unwrap_or_default().to_string() + ":",
    }
}

fn with_params(uri: &str, pairs: &[(&str, &str)]) -> String {
    let query = pairs
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{query}")
}

fn with_owned_params(uri: &str, pairs: &[(&str, String)]) -> String {
    let borrowed: Vec<(&str, &str)> = pairs.iter().map(|(n, v)| (*n, v.as_str())).collect();
    with_params(uri, &borrowed)
}

/// Percent-encodes everything but unreserved characters.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// A page for problems that cannot be sent back to the client. The message
/// may contain a client's self-chosen name, so it is escaped.
fn error_page(message: &str) -> Response {
    let message = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    (
        StatusCode::BAD_REQUEST,
        Html(format!(
            "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\">\
             <title>Cannot connect</title><main style=\"font-family:system-ui;max-width:32rem;margin:4rem auto;padding:0 1rem\">\
             <h1>Cannot connect to Vulse</h1><p>{message}</p></main>"
        )),
    )
        .into_response()
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn redirects_are_limited_to_safe_targets() {
        for uri in [
            "https://claude.ai/api/mcp/auth_callback",
            "http://localhost:33418/callback",
            "http://127.0.0.1/cb",
            "http://[::1]:8000/cb",
            "cursor://anysphere.cursor-mcp/oauth/callback",
        ] {
            assert!(valid_redirect(uri), "{uri}");
        }
        for uri in [
            "http://example.com/cb",
            "https://claude.ai/cb#frag",
            "javascript:alert(1)",
            "https://user@evil.example/cb",
            "https:///cb",
            "data:text/html,hi",
        ] {
            assert!(!valid_redirect(uri), "{uri}");
        }
    }

    #[test]
    fn loopback_redirects_may_change_port() {
        assert!(redirect_matches(
            "http://localhost:1000/cb",
            "http://localhost:2000/cb"
        ));
        assert!(!redirect_matches(
            "http://localhost:1000/cb",
            "http://localhost:2000/other"
        ));
        assert!(!redirect_matches(
            "https://a.example/cb",
            "https://a.example:444/cb"
        ));
    }

    #[test]
    fn scopes_default_to_read_and_write() {
        assert_eq!(parse_scope(None).unwrap(), "records:read records:write");
        assert_eq!(parse_scope(Some("records:read")).unwrap(), "records:read");
        assert_eq!(
            parse_scope(Some("records:write")).unwrap(),
            "records:read records:write"
        );
        assert!(parse_scope(Some("admin")).is_none());
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(
            with_params("https://a.example/cb?x=1", &[("state", "a b&c")]),
            "https://a.example/cb?x=1&state=a%20b%26c"
        );
    }
}
