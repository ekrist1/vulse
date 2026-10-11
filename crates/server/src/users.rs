use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use vulse_model::{
    CreateInvite, CreatedInvite, InviteView, LinkPurpose, ManagedUser, Role, UpdateUser,
    check_email,
};

use crate::auth::{AdminSession, hash_secret, new_id, new_token};
use crate::db::Conn;
use crate::error::AppError;
use crate::state::AppState;
use crate::store;
use crate::timeutil;

pub async fn list_users(
    State(state): State<AppState>,
    AdminSession(_admin): AdminSession,
) -> Result<Json<Vec<ManagedUser>>, AppError> {
    let conn = state.db.connect().await?;
    Ok(Json(store::list_users(&conn).await?))
}

/// Changes a role or disables an account. Disabling ends its sessions and
/// stops its API keys. The workspace always keeps one active admin.
pub async fn update_user(
    State(state): State<AppState>,
    AdminSession(_admin): AdminSession,
    Path(id): Path<String>,
    Json(body): Json<UpdateUser>,
) -> Result<Json<ManagedUser>, AppError> {
    let conn = state.db.connect().await?;
    conn.with_app_tx(async |conn| {
        let user = store::find_user(conn, &id)
            .await?
            .ok_or(AppError::NotFound)?;
        if let Some(role) = body.role {
            if role != user.role {
                store::set_user_role(conn, &id, role).await?;
            }
        }
        if let Some(disabled) = body.disabled {
            if disabled && !user.disabled {
                store::set_user_disabled(conn, &id, Some(&timeutil::now_rfc3339())).await?;
                store::delete_user_sessions(conn, &id, None).await?;
            } else if !disabled && user.disabled {
                store::set_user_disabled(conn, &id, None).await?;
            }
        }
        if store::count_active_admins(conn).await? == 0 {
            return Err(AppError::conflict(
                "the workspace needs at least one active admin",
            ));
        }
        Ok(())
    })
    .await?;
    store::list_users(&conn)
        .await?
        .into_iter()
        .find(|user| user.id == id)
        .map(Json)
        .ok_or(AppError::NotFound)
}

/// Issues a single-use link that lets an existing user choose a new password.
pub async fn reset_password(
    State(state): State<AppState>,
    AdminSession(admin): AdminSession,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<CreatedInvite>), AppError> {
    let conn = state.db.connect().await?;
    let user = store::find_user(&conn, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    if user.disabled {
        return Err(AppError::conflict(
            "enable the account before resetting its password",
        ));
    }
    let created = create_link(
        &conn,
        LinkPurpose::Reset,
        &user.email,
        user.role,
        Some(&user.id),
        &admin.user_id,
        timeutil::RESET_SECONDS,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub async fn list_invites(
    State(state): State<AppState>,
    AdminSession(_admin): AdminSession,
) -> Result<Json<Vec<InviteView>>, AppError> {
    let conn = state.db.connect().await?;
    Ok(Json(store::list_links(&conn, &timeutil::now_secs()).await?))
}

pub async fn create_invite(
    State(state): State<AppState>,
    AdminSession(admin): AdminSession,
    Json(body): Json<CreateInvite>,
) -> Result<(StatusCode, Json<CreatedInvite>), AppError> {
    let email = body.email.trim().to_ascii_lowercase();
    check_email(&email).map_err(AppError::bad)?;
    let conn = state.db.connect().await?;
    if store::find_user_by_email(&conn, &email).await?.is_some() {
        return Err(AppError::conflict("email already registered"));
    }
    let created = create_link(
        &conn,
        LinkPurpose::Invite,
        &email,
        body.role,
        None,
        &admin.user_id,
        timeutil::INVITE_SECONDS,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub async fn delete_invite(
    State(state): State<AppState>,
    AdminSession(_admin): AdminSession,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let conn = state.db.connect().await?;
    if store::delete_link(&conn, &id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

/// The token goes in the URL fragment, which browsers do not send to servers
/// or in `Referer` headers.
async fn create_link(
    conn: &Conn,
    purpose: LinkPurpose,
    email: &str,
    role: Role,
    user_id: Option<&str>,
    created_by: &str,
    lifetime: i64,
) -> Result<CreatedInvite, AppError> {
    let id = new_id();
    let token = new_token();
    conn.with_app_tx(async |conn| {
        store::delete_links_for_email(conn, email, purpose).await?;
        store::insert_link(
            conn,
            &id,
            &hash_secret(&token),
            purpose,
            email,
            role,
            user_id,
            created_by,
            &timeutil::now_secs(),
            &timeutil::expiry_in(lifetime),
        )
        .await?;
        Ok(())
    })
    .await?;
    let invite = store::find_link_view(conn, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("created link disappeared"))?;
    Ok(CreatedInvite {
        invite,
        path: format!("/accept#token={token}"),
        token,
    })
}
