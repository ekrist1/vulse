use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::header::{self, HeaderValue};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use vulse_model::{
    CreateApiKey, CreatedApiKey, LoginRequest, RegisterRequest, UserView, check_email,
    check_handle, check_key_name, check_password,
};

use crate::db::Conn;
use crate::error::{AppError, unique_message};
use crate::state::AppState;
use crate::store;
use crate::timeutil::{self, SESSION_SECONDS};

pub const COOKIE_NAME: &str = "vulse_session";

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
    pub via: AuthVia,
    pub collection_scope: Option<String>,
    pub session_token: Option<String>,
}

pub struct AuthUser(pub Principal);
pub struct AccountOnly(pub Principal);
pub struct SessionOnly(pub Principal);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(AuthUser(authenticate(state, parts).await?))
    }
}

impl FromRequestParts<AppState> for AccountOnly {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let principal = authenticate(state, parts).await?;
        if principal.collection_scope.is_some() {
            return Err(AppError::Forbidden(
                "this api key is limited to one collection".into(),
            ));
        }
        Ok(AccountOnly(principal))
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
                "api key management requires a session".into(),
            ));
        }
        Ok(SessionOnly(principal))
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

pub async fn register(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<Response, AppError> {
    let email = body.email.trim().to_ascii_lowercase();
    let handle = body.handle.trim().to_ascii_lowercase();
    check_email(&email).map_err(AppError::bad)?;
    check_handle(&handle).map_err(AppError::bad)?;
    check_password(&body.password).map_err(AppError::bad)?;
    let permit = state
        .password_work
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests)?;
    let password_hash = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hash_password(&body.password)
    })
    .await
    .map_err(anyhow::Error::from)??;
    let conn = state.db.connect().await?;
    let user_id = new_id();
    let created_at = timeutil::now_rfc3339();
    conn.with_tx(async |conn| {
        store::insert_user(conn, &user_id, &handle, &email, &password_hash, &created_at).await
    })
    .await
    .map_err(|err| unique_message(err, "email or handle already registered"))?;
    let token = new_token();
    store::insert_session(&conn, &token, &user_id, &timeutil::session_expiry()).await?;
    let user = UserView {
        id: user_id,
        email,
        handle,
    };
    Ok(json_cookie(
        StatusCode::CREATED,
        &user,
        &token,
        SESSION_SECONDS,
    ))
}

pub async fn login(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let email = body.email.trim().to_ascii_lowercase();
    let conn = state.db.connect().await?;
    let Some(user) = store::find_user_by_email(&conn, &email).await? else {
        return Err(AppError::Unauthorized);
    };
    if check_password(&body.password).is_err() {
        return Err(AppError::Unauthorized);
    }
    let permit = state
        .password_work
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests)?;
    let encoded = user.password_hash.clone();
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        verify_password(&body.password, &encoded)
    })
    .await
    .map_err(anyhow::Error::from)?;
    if !valid {
        return Err(AppError::Unauthorized);
    }
    let token = new_token();
    store::insert_session(&conn, &token, &user.id, &timeutil::session_expiry()).await?;
    let view = UserView {
        id: user.id,
        email: user.email,
        handle: user.handle,
    };
    Ok(json_cookie(StatusCode::OK, &view, &token, SESSION_SECONDS))
}

pub async fn logout(
    axum::extract::State(state): axum::extract::State<AppState>,
    SessionOnly(principal): SessionOnly,
) -> Result<Response, AppError> {
    if let Some(token) = principal.session_token {
        let conn = state.db.connect().await?;
        store::delete_session(&conn, &token).await?;
    }
    Ok(clear_cookie())
}

pub async fn me(AuthUser(principal): AuthUser) -> Json<UserView> {
    Json(UserView {
        id: principal.user_id,
        email: principal.email,
        handle: principal.handle,
    })
}

pub async fn list_keys(
    axum::extract::State(state): axum::extract::State<AppState>,
    SessionOnly(principal): SessionOnly,
) -> Result<Json<Vec<vulse_model::ApiKeyView>>, AppError> {
    let conn = state.db.connect().await?;
    Ok(Json(store::list_api_keys(&conn, &principal.user_id).await?))
}

pub async fn create_key(
    axum::extract::State(state): axum::extract::State<AppState>,
    SessionOnly(principal): SessionOnly,
    Json(body): Json<CreateApiKey>,
) -> Result<(StatusCode, Json<CreatedApiKey>), AppError> {
    let name = body.name.trim();
    check_key_name(name).map_err(AppError::bad)?;
    let conn = state.db.connect().await?;
    if let Some(collection_id) = &body.collection_id {
        if store::find_collection(&conn, &principal.user_id, collection_id)
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
        &hash_key(&secret),
        body.collection_id.as_deref(),
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
    axum::extract::State(state): axum::extract::State<AppState>,
    SessionOnly(principal): SessionOnly,
    axum::extract::Path(id): axum::extract::Path<String>,
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
    if let Some(token) = cookie_token(parts) {
        return session_principal(&conn, &token).await;
    }
    Err(AppError::Unauthorized)
}

async fn session_principal(conn: &Conn, token: &str) -> Result<Principal, AppError> {
    let Some(session) = store::find_session(conn, token).await? else {
        return Err(AppError::Unauthorized);
    };
    if timeutil::is_expired(&session.expires_at) {
        store::delete_session(conn, token).await?;
        return Err(AppError::Unauthorized);
    }
    Ok(Principal {
        user_id: session.user_id,
        email: session.email,
        handle: session.handle,
        via: AuthVia::Session,
        collection_scope: None,
        session_token: Some(token.to_string()),
    })
}

async fn api_key_principal(conn: &Conn, secret: &str) -> Result<Principal, AppError> {
    let Some(key) = store::find_api_key(conn, &hash_key(secret)).await? else {
        return Err(AppError::Unauthorized);
    };
    Ok(Principal {
        user_id: key.user_id,
        email: key.email,
        handle: key.handle,
        via: AuthVia::ApiKey,
        collection_scope: key.collection_id,
        session_token: None,
    })
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

fn hash_key(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn new_token() -> String {
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

fn cookie_token(parts: &Parts) -> Option<String> {
    cookie_from_headers(&parts.headers)
}

fn cookie_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        if let Some(value) = part.trim().strip_prefix(&format!("{COOKIE_NAME}=")) {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn json_cookie<T: serde::Serialize>(
    status: StatusCode,
    body: &T,
    token: &str,
    max_age: i64,
) -> Response {
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie(token, max_age));
    response
}

fn clear_cookie() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie("", 0));
    response
}

fn session_cookie(token: &str, max_age: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}"
    ))
    .expect("session cookie is valid")
}
