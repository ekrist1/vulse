use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::Json;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderValue};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use vulse_model::{
    AcceptLink, ChangePassword, CreateApiKey, CreatedApiKey, LinkInfo, LinkPurpose,
    LinkTokenRequest, LoginRequest, Role, SetupRequest, SetupStatus, UserView, check_email,
    check_handle, check_key_name, check_password,
};

use crate::db::Conn;
use crate::error::{AppError, unique_message};
use crate::security::{ACCOUNT_LIMIT, ADDRESS_LIMIT, ClientIp, account_key, address_key};
use crate::state::AppState;
use crate::store;
use crate::timeutil::{self, SESSION_SECONDS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthVia {
    Session,
    ApiKey,
}

#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: String,
    pub email: String,
    pub handle: String,
    /// For API keys, the lower of the key's role and its owner's current role.
    pub role: Role,
    pub via: AuthVia,
    pub collection_scope: Option<String>,
    pub session_hash: Option<String>,
}

impl Principal {
    pub fn view(&self) -> UserView {
        UserView {
            id: self.user_id.clone(),
            email: self.email.clone(),
            handle: self.handle.clone(),
            role: self.role,
        }
    }
}

/// Any signed-in account or API key. Viewers can read everything.
pub struct AuthUser(pub Principal);
/// Editors and admins: record, asset, import and saved view writes.
pub struct Editor(pub Principal);
/// Admins without a collection scope: collection create, update and delete.
pub struct SchemaAdmin(pub Principal);
/// A browser session, never an API key.
pub struct SessionOnly(pub Principal);
/// An admin's browser session: user and invite management.
pub struct AdminSession(pub Principal);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(AuthUser(authenticate(state, parts).await?))
    }
}

impl FromRequestParts<AppState> for Editor {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let principal = authenticate(state, parts).await?;
        require_role(&principal, Role::Editor)?;
        Ok(Editor(principal))
    }
}

impl FromRequestParts<AppState> for SchemaAdmin {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let principal = authenticate(state, parts).await?;
        require_role(&principal, Role::Admin)?;
        if principal.collection_scope.is_some() {
            return Err(AppError::Forbidden(
                "this api key is limited to one collection".into(),
            ));
        }
        Ok(SchemaAdmin(principal))
    }
}

impl FromRequestParts<AppState> for SessionOnly {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let principal = authenticate(state, parts).await?;
        if principal.via != AuthVia::Session {
            return Err(AppError::Forbidden(
                "this action requires a signed-in session".into(),
            ));
        }
        Ok(SessionOnly(principal))
    }
}

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let SessionOnly(principal) = SessionOnly::from_request_parts(parts, state).await?;
        require_role(&principal, Role::Admin)?;
        Ok(AdminSession(principal))
    }
}

pub fn require_role(principal: &Principal, role: Role) -> Result<(), AppError> {
    if principal.role >= role {
        Ok(())
    } else {
        Err(AppError::Forbidden(format!(
            "this action requires the {} role",
            role.as_str()
        )))
    }
}

pub fn require_collection(principal: &Principal, collection_id: &str) -> Result<(), AppError> {
    if let Some(scope) = &principal.collection_scope {
        if scope != collection_id {
            return Err(AppError::Forbidden(
                "this api key is limited to one collection".into(),
            ));
        }
    }
    Ok(())
}

pub async fn setup_status(State(state): State<AppState>) -> Result<Json<SetupStatus>, AppError> {
    let conn = state.db.connect().await?;
    Ok(Json(SetupStatus {
        needs_setup: store::count_users(&conn).await? == 0,
    }))
}

/// Creates the first admin with the setup token printed at startup.
pub async fn setup(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<SetupRequest>,
) -> Result<Response, AppError> {
    let address = address_key(&ip);
    state.throttle.check(&[(&address, ADDRESS_LIMIT)])?;
    if hash_secret(body.token.trim()) != state.auth.setup_token_hash {
        state.throttle.fail(&[&address]);
        return Err(AppError::Forbidden("the setup token is not valid".into()));
    }
    let email = body.email.trim().to_ascii_lowercase();
    let handle = body.handle.trim().to_ascii_lowercase();
    check_email(&email).map_err(AppError::bad)?;
    check_handle(&handle).map_err(AppError::bad)?;
    check_password(&body.password).map_err(AppError::bad)?;
    let conn = state.db.connect().await?;
    if store::count_users(&conn).await? > 0 {
        return Err(AppError::conflict("the workspace is already set up"));
    }
    let password_hash = hash_on_worker(&state, body.password).await?;
    let user_id = new_id();
    conn.with_app_tx(async |conn| {
        if store::count_users(conn).await? > 0 {
            return Err(AppError::conflict("the workspace is already set up"));
        }
        store::insert_user(
            conn,
            &user_id,
            &handle,
            &email,
            &password_hash,
            Role::Admin,
            &timeutil::now_rfc3339(),
        )
        .await?;
        Ok(())
    })
    .await?;
    tracing::info!(%email, "created the first admin");
    let user = UserView {
        id: user_id,
        email,
        handle,
        role: Role::Admin,
    };
    start_session(&state, &conn, user, StatusCode::CREATED).await
}

pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let email = body.email.trim().to_ascii_lowercase();
    let account = account_key(&email);
    let address = address_key(&ip);
    state
        .throttle
        .check(&[(&account, ACCOUNT_LIMIT), (&address, ADDRESS_LIMIT)])?;
    if check_password(&body.password).is_err() {
        state.throttle.fail(&[&account, &address]);
        return Err(AppError::Unauthorized);
    }
    let conn = state.db.connect().await?;
    let user = store::find_user_by_email(&conn, &email).await?;
    // Unknown addresses are checked against a fixed hash so the response time
    // does not reveal which accounts exist.
    let encoded = user.as_ref().map(|user| user.password_hash.clone());
    let valid = verify_on_worker(&state, body.password, encoded).await?;
    let Some(user) = user.filter(|_| valid) else {
        state.throttle.fail(&[&account, &address]);
        return Err(AppError::Unauthorized);
    };
    if user.disabled {
        return Err(AppError::Forbidden("this account is disabled".into()));
    }
    state.throttle.clear(&account);
    let view = UserView {
        id: user.id,
        email: user.email,
        handle: user.handle,
        role: user.role,
    };
    start_session(&state, &conn, view, StatusCode::OK).await
}

pub async fn logout(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
) -> Result<Response, AppError> {
    if let Some(hash) = principal.session_hash {
        let conn = state.db.connect().await?;
        store::delete_session(&conn, &hash).await?;
    }
    Ok(clear_cookie(&state))
}

/// Ends every session of the signed-in account, including this one.
pub async fn logout_everywhere(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
) -> Result<Response, AppError> {
    let conn = state.db.connect().await?;
    store::delete_user_sessions(&conn, &principal.user_id, None).await?;
    Ok(clear_cookie(&state))
}

/// Changes the password and ends the account's other sessions.
pub async fn change_password(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
    Json(body): Json<ChangePassword>,
) -> Result<StatusCode, AppError> {
    let account = account_key(&principal.email);
    state.throttle.check(&[(&account, ACCOUNT_LIMIT)])?;
    check_password(&body.new_password).map_err(AppError::bad)?;
    let conn = state.db.connect().await?;
    let user = store::find_user(&conn, &principal.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if !verify_on_worker(&state, body.current_password, Some(user.password_hash)).await? {
        state.throttle.fail(&[&account]);
        return Err(AppError::bad("the current password is not correct"));
    }
    let password_hash = hash_on_worker(&state, body.new_password).await?;
    conn.with_app_tx(async |conn| {
        store::set_password_hash(conn, &principal.user_id, &password_hash).await?;
        store::delete_user_sessions(conn, &principal.user_id, principal.session_hash.as_deref())
            .await?;
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Tells the accept page whether a link is an invite or a reset, and for whom.
pub async fn link_info(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<LinkTokenRequest>,
) -> Result<Json<LinkInfo>, AppError> {
    let conn = state.db.connect().await?;
    let link = valid_link(&state, &conn, &ip, &body.token).await?;
    Ok(Json(LinkInfo {
        purpose: link.purpose,
        email: link.email,
    }))
}

/// Completes an invite or a password reset and signs the account in.
pub async fn accept_link(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<AcceptLink>,
) -> Result<Response, AppError> {
    let conn = state.db.connect().await?;
    let link = valid_link(&state, &conn, &ip, &body.token).await?;
    check_password(&body.password).map_err(AppError::bad)?;
    let handle = match link.purpose {
        LinkPurpose::Invite => {
            let handle = body
                .handle
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            check_handle(&handle).map_err(AppError::bad)?;
            Some(handle)
        }
        LinkPurpose::Reset => None,
    };
    let password_hash = hash_on_worker(&state, body.password).await?;
    let token_hash = hash_secret(body.token.trim());
    let user = conn
        .with_app_tx(async |conn| {
            // Re-read inside the transaction so a link is used at most once.
            let link = store::find_link(conn, &token_hash)
                .await?
                .filter(|link| !timeutil::is_expired(&link.expires_at))
                .ok_or_else(expired_link)?;
            store::delete_link(conn, &link.id).await?;
            match (link.purpose, &handle) {
                (LinkPurpose::Invite, Some(handle)) => {
                    let id = new_id();
                    store::insert_user(
                        conn,
                        &id,
                        handle,
                        &link.email,
                        &password_hash,
                        link.role,
                        &timeutil::now_rfc3339(),
                    )
                    .await
                    .map_err(|err| unique_message(err, "email or handle already registered"))?;
                    Ok(UserView {
                        id,
                        email: link.email,
                        handle: handle.clone(),
                        role: link.role,
                    })
                }
                _ => {
                    let user = store::find_user(conn, link.user_id.as_deref().unwrap_or(""))
                        .await?
                        .ok_or_else(expired_link)?;
                    if user.disabled {
                        return Err(AppError::Forbidden("this account is disabled".into()));
                    }
                    store::set_password_hash(conn, &user.id, &password_hash).await?;
                    store::delete_user_sessions(conn, &user.id, None).await?;
                    Ok(UserView {
                        id: user.id,
                        email: user.email,
                        handle: user.handle,
                        role: user.role,
                    })
                }
            }
        })
        .await?;
    start_session(&state, &conn, user, StatusCode::OK).await
}

async fn valid_link(
    state: &AppState,
    conn: &Conn,
    ip: &str,
    token: &str,
) -> Result<store::LinkRecord, AppError> {
    let address = address_key(ip);
    state.throttle.check(&[(&address, ADDRESS_LIMIT)])?;
    match store::find_link(conn, &hash_secret(token.trim())).await? {
        Some(link) if !timeutil::is_expired(&link.expires_at) => Ok(link),
        _ => {
            state.throttle.fail(&[&address]);
            Err(expired_link())
        }
    }
}

fn expired_link() -> AppError {
    AppError::bad("this link is not valid. It may have expired or already been used.")
}

pub async fn me(AuthUser(principal): AuthUser) -> Json<UserView> {
    Json(principal.view())
}

pub async fn list_keys(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
) -> Result<Json<Vec<vulse_model::ApiKeyView>>, AppError> {
    let conn = state.db.connect().await?;
    Ok(Json(store::list_api_keys(&conn, &principal.user_id).await?))
}

pub async fn create_key(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
    Json(body): Json<CreateApiKey>,
) -> Result<(StatusCode, Json<CreatedApiKey>), AppError> {
    let name = body.name.trim();
    check_key_name(name).map_err(AppError::bad)?;
    let role = body.role.unwrap_or(principal.role);
    if role > principal.role {
        return Err(AppError::Forbidden(
            "a key cannot have more access than your account".into(),
        ));
    }
    let conn = state.db.connect().await?;
    if let Some(collection_id) = &body.collection_id {
        if store::find_collection(&conn, collection_id)
            .await?
            .is_none()
        {
            return Err(AppError::NotFound);
        }
    }
    let secret = format!("vulse_{}", new_token());
    let prefix = secret.chars().take(16).collect::<String>();
    let id = new_id();
    store::insert_api_key(
        &conn,
        &id,
        &principal.user_id,
        name,
        &prefix,
        &hash_secret(&secret),
        body.collection_id.as_deref(),
        role,
        &timeutil::now_rfc3339(),
    )
    .await?;
    let key = store::list_api_keys(&conn, &principal.user_id)
        .await?
        .into_iter()
        .find(|key| key.id == id)
        .ok_or_else(|| anyhow::anyhow!("created api key disappeared"))?;
    Ok((StatusCode::CREATED, Json(CreatedApiKey { key, secret })))
}

pub async fn delete_key(
    State(state): State<AppState>,
    SessionOnly(principal): SessionOnly,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let conn = state.db.connect().await?;
    if store::delete_api_key(&conn, &principal.user_id, &id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

async fn authenticate(state: &AppState, parts: &Parts) -> Result<Principal, AppError> {
    let conn = state.db.connect().await?;
    if let Some(token) = bearer_token(parts) {
        return api_key_principal(&conn, &token).await;
    }
    if let Some(token) = cookie_from_headers(&parts.headers, state.auth.cookie_name()) {
        return session_principal(&conn, &token).await;
    }
    Err(AppError::Unauthorized)
}

async fn session_principal(conn: &Conn, token: &str) -> Result<Principal, AppError> {
    let hash = hash_secret(token);
    let Some(session) = store::find_session(conn, &hash).await? else {
        return Err(AppError::Unauthorized);
    };
    if timeutil::is_expired(&session.expires_at) {
        store::delete_session(conn, &hash).await?;
        return Err(AppError::Unauthorized);
    }
    Ok(Principal {
        user_id: session.user_id,
        email: session.email,
        handle: session.handle,
        role: session.role,
        via: AuthVia::Session,
        collection_scope: None,
        session_hash: Some(hash),
    })
}

async fn api_key_principal(conn: &Conn, secret: &str) -> Result<Principal, AppError> {
    let Some(key) = store::find_api_key(conn, &hash_secret(secret)).await? else {
        return Err(AppError::Unauthorized);
    };
    Ok(Principal {
        user_id: key.user_id,
        email: key.email,
        handle: key.handle,
        role: key.key_role.min(key.user_role),
        via: AuthVia::ApiKey,
        collection_scope: key.collection_id,
        session_hash: None,
    })
}

async fn start_session(
    state: &AppState,
    conn: &Conn,
    user: UserView,
    status: StatusCode,
) -> Result<Response, AppError> {
    let token = new_token();
    store::insert_session(
        conn,
        &hash_secret(&token),
        &user.id,
        &timeutil::now_secs(),
        &timeutil::session_expiry(),
    )
    .await?;
    let mut response = (status, Json(user)).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(state, &token, SESSION_SECONDS),
    );
    Ok(response)
}

/// Waits briefly for one of the bounded hashing slots, then hashes off the
/// async runtime.
pub(crate) async fn hash_on_worker(state: &AppState, password: String) -> Result<String, AppError> {
    let permit = password_slot(state).await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hash_password(&password)
    })
    .await
    .map_err(anyhow::Error::from)?
}

async fn verify_on_worker(
    state: &AppState,
    password: String,
    encoded: Option<String>,
) -> Result<bool, AppError> {
    let permit = password_slot(state).await?;
    Ok(tokio::task::spawn_blocking(move || {
        let _permit = permit;
        match encoded {
            Some(encoded) => verify_password(&password, &encoded),
            None => {
                verify_password(&password, dummy_hash());
                false
            }
        }
    })
    .await
    .map_err(anyhow::Error::from)?)
}

async fn password_slot(state: &AppState) -> Result<tokio::sync::OwnedSemaphorePermit, AppError> {
    tokio::time::timeout(
        state.auth.password_wait,
        state.password_work.clone().acquire_owned(),
    )
    .await
    .map_err(|_| AppError::TooManyRequests)?
    .map_err(|_| AppError::TooManyRequests)
}

fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| hash_password("vulse-unknown-account").expect("hash dummy password"))
}

fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| AppError::Internal(anyhow::anyhow!(err.to_string())))
}

fn verify_password(password: &str, encoded: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(encoded) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// SHA-256 for high-entropy secrets: session tokens, API keys and links.
pub(crate) fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub(crate) fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn bearer_token(parts: &Parts) -> Option<String> {
    let value = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let token = token.trim();
        if token.is_empty() {
            None
        } else {
            Some(token.to_string())
        }
    } else {
        None
    }
}

fn cookie_from_headers(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        if let Some(value) = part
            .trim()
            .strip_prefix(name)
            .and_then(|v| v.strip_prefix('='))
        {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn clear_cookie(state: &AppState) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie(state, "", 0));
    response
}

fn session_cookie(state: &AppState, token: &str, max_age: i64) -> HeaderValue {
    let name = state.auth.cookie_name();
    let secure = if state.auth.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{name}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}"
    ))
    .expect("session cookie is valid")
}
