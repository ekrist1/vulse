use vulse_model::{
    ApiKeyView, AssetView, Blueprint, CollectionView, DocumentView, InviteView, LinkPurpose,
    ManagedUser, Role, Visibility,
};

use crate::db::{Conn, SqlRow, SqlValue, int_at, opt_text_at, text_at};

pub struct UserRecord {
    pub id: String,
    pub handle: String,
    pub email: String,
    pub password_hash: String,
    pub role: Role,
    pub disabled: bool,
}

pub struct SessionRecord {
    pub user_id: String,
    pub email: String,
    pub handle: String,
    pub role: Role,
    pub expires_at: String,
}

pub struct ApiKeyRecord {
    pub user_id: String,
    pub email: String,
    pub handle: String,
    pub user_role: Role,
    pub key_role: Role,
    pub collection_id: Option<String>,
}

pub struct LinkRecord {
    pub id: String,
    pub purpose: LinkPurpose,
    pub email: String,
    pub role: Role,
    pub user_id: Option<String>,
    pub expires_at: String,
}

pub struct AssetRecord {
    pub view: AssetView,
    pub storage_key: String,
}

pub async fn insert_user(
    conn: &Conn,
    id: &str,
    handle: &str,
    email: &str,
    password_hash: &str,
    role: Role,
    created_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO users (id, handle, email, password_hash, role, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        &[
            text(id),
            text(handle),
            text(email),
            text(password_hash),
            text(role.as_str()),
            text(created_at),
        ],
    )
    .await?;
    Ok(())
}

pub async fn count_users(conn: &Conn) -> anyhow::Result<i64> {
    let rows = conn.query("SELECT COUNT(*) FROM users", &[]).await?;
    rows.first().map(|row| int_at(row, 0)).unwrap_or(Ok(0))
}

const USER_COLUMNS: &str = "id, handle, email, password_hash, role, disabled_at";

pub async fn find_user_by_email(conn: &Conn, email: &str) -> anyhow::Result<Option<UserRecord>> {
    let rows = conn
        .query(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE email = ?1"),
            &[text(email)],
        )
        .await?;
    optional_row(rows, user_from_row)
}

pub async fn find_user(conn: &Conn, id: &str) -> anyhow::Result<Option<UserRecord>> {
    let rows = conn
        .query(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
            &[text(id)],
        )
        .await?;
    optional_row(rows, user_from_row)
}

pub async fn list_users(conn: &Conn) -> anyhow::Result<Vec<ManagedUser>> {
    let rows = conn
        .query(
            "SELECT id, email, handle, role, disabled_at, created_at
             FROM users ORDER BY created_at, id",
            &[],
        )
        .await?;
    rows.iter()
        .map(|row| {
            Ok(ManagedUser {
                id: text_at(row, 0)?,
                email: text_at(row, 1)?,
                handle: text_at(row, 2)?,
                role: role_at(row, 3)?,
                disabled_at: opt_text_at(row, 4)?,
                created_at: text_at(row, 5)?,
            })
        })
        .collect()
}

/// Admins that can still sign in. The workspace must always keep one.
pub async fn count_active_admins(conn: &Conn) -> anyhow::Result<i64> {
    let rows = conn
        .query(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled_at IS NULL",
            &[],
        )
        .await?;
    rows.first().map(|row| int_at(row, 0)).unwrap_or(Ok(0))
}

pub async fn set_user_role(conn: &Conn, id: &str, role: Role) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE users SET role = ?1 WHERE id = ?2",
        &[text(role.as_str()), text(id)],
    )
    .await?;
    Ok(())
}

pub async fn set_user_disabled(
    conn: &Conn,
    id: &str,
    disabled_at: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE users SET disabled_at = ?1 WHERE id = ?2",
        &[disabled_at.map(text).unwrap_or(SqlValue::Null), text(id)],
    )
    .await?;
    Ok(())
}

pub async fn set_password_hash(conn: &Conn, id: &str, password_hash: &str) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        &[text(password_hash), text(id)],
    )
    .await?;
    Ok(())
}

pub async fn insert_session(
    conn: &Conn,
    token_hash: &str,
    user_id: &str,
    created_at: &str,
    expires_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO auth_sessions (token_hash, user_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
        &[
            text(token_hash),
            text(user_id),
            text(created_at),
            text(expires_at),
        ],
    )
    .await?;
    Ok(())
}

/// Finds a session whose account is still enabled.
pub async fn find_session(conn: &Conn, token_hash: &str) -> anyhow::Result<Option<SessionRecord>> {
    let rows = conn
        .query(
            "SELECT s.user_id, s.expires_at, u.email, u.handle, u.role
             FROM auth_sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND u.disabled_at IS NULL",
            &[text(token_hash)],
        )
        .await?;
    optional_row(rows, |row| {
        Ok(SessionRecord {
            user_id: text_at(row, 0)?,
            expires_at: text_at(row, 1)?,
            email: text_at(row, 2)?,
            handle: text_at(row, 3)?,
            role: role_at(row, 4)?,
        })
    })
}

pub async fn delete_session(conn: &Conn, token_hash: &str) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM auth_sessions WHERE token_hash = ?1",
        &[text(token_hash)],
    )
    .await?;
    Ok(())
}

/// Ends every session of a user except `keep`, if given, and disconnects
/// the apps they connected through OAuth.
pub async fn delete_user_sessions(
    conn: &Conn,
    user_id: &str,
    keep: Option<&str>,
) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM auth_sessions WHERE user_id = ?1 AND (?2 IS NULL OR token_hash <> ?2)",
        &[text(user_id), keep.map(text).unwrap_or(SqlValue::Null)],
    )
    .await?;
    crate::oauth::revoke_user(conn, user_id).await
}

/// Removes expired sessions, account links and OAuth codes and tokens.
/// Timestamps share one fixed-width UTC format, so text comparison orders
/// them correctly.
pub async fn purge_expired(conn: &Conn, now: &str) -> anyhow::Result<u64> {
    let sessions = conn
        .execute(
            "DELETE FROM auth_sessions WHERE expires_at < ?1",
            &[text(now)],
        )
        .await?;
    let links = conn
        .execute(
            "DELETE FROM account_links WHERE expires_at < ?1",
            &[text(now)],
        )
        .await?;
    let oauth = crate::oauth::purge(conn, now).await?;
    Ok(sessions + links + oauth)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_link(
    conn: &Conn,
    id: &str,
    token_hash: &str,
    purpose: LinkPurpose,
    email: &str,
    role: Role,
    user_id: Option<&str>,
    created_by: &str,
    created_at: &str,
    expires_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO account_links
         (id, token_hash, purpose, email, role, user_id, created_by, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        &[
            text(id),
            text(token_hash),
            text(purpose_str(purpose)),
            text(email),
            text(role.as_str()),
            user_id.map(text).unwrap_or(SqlValue::Null),
            text(created_by),
            text(created_at),
            text(expires_at),
        ],
    )
    .await?;
    Ok(())
}

pub async fn find_link(conn: &Conn, token_hash: &str) -> anyhow::Result<Option<LinkRecord>> {
    let rows = conn
        .query(
            "SELECT id, purpose, email, role, user_id, expires_at
             FROM account_links WHERE token_hash = ?1",
            &[text(token_hash)],
        )
        .await?;
    optional_row(rows, |row| {
        Ok(LinkRecord {
            id: text_at(row, 0)?,
            purpose: purpose_at(row, 1)?,
            email: text_at(row, 2)?,
            role: role_at(row, 3)?,
            user_id: opt_text_at(row, 4)?,
            expires_at: text_at(row, 5)?,
        })
    })
}

pub async fn list_links(conn: &Conn, now: &str) -> anyhow::Result<Vec<InviteView>> {
    let rows = conn
        .query(
            "SELECT id, purpose, email, role, created_at, expires_at
             FROM account_links WHERE expires_at >= ?1
             ORDER BY created_at DESC, id DESC",
            &[text(now)],
        )
        .await?;
    rows.iter().map(link_view_from_row).collect()
}

pub async fn find_link_view(conn: &Conn, id: &str) -> anyhow::Result<Option<InviteView>> {
    let rows = conn
        .query(
            "SELECT id, purpose, email, role, created_at, expires_at
             FROM account_links WHERE id = ?1",
            &[text(id)],
        )
        .await?;
    optional_row(rows, link_view_from_row)
}

pub async fn delete_link(conn: &Conn, id: &str) -> anyhow::Result<bool> {
    let changed = conn
        .execute("DELETE FROM account_links WHERE id = ?1", &[text(id)])
        .await?;
    Ok(changed > 0)
}

/// A newer invite or reset for the same address replaces older ones.
pub async fn delete_links_for_email(
    conn: &Conn,
    email: &str,
    purpose: LinkPurpose,
) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM account_links WHERE email = ?1 AND purpose = ?2",
        &[text(email), text(purpose_str(purpose))],
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_api_key(
    conn: &Conn,
    id: &str,
    user_id: &str,
    name: &str,
    prefix: &str,
    key_hash: &str,
    collection_id: Option<&str>,
    role: Role,
    created_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO api_keys (id, user_id, name, key_prefix, key_hash, collection_id, role, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        &[
            text(id),
            text(user_id),
            text(name),
            text(prefix),
            text(key_hash),
            collection_id.map(text).unwrap_or(SqlValue::Null),
            text(role.as_str()),
            text(created_at),
        ],
    )
    .await?;
    Ok(())
}

pub async fn list_api_keys(conn: &Conn, user_id: &str) -> anyhow::Result<Vec<ApiKeyView>> {
    let rows = conn
        .query(
            "SELECT id, name, key_prefix, collection_id, role, created_at
             FROM api_keys WHERE user_id = ?1 ORDER BY created_at DESC, id DESC",
            &[text(user_id)],
        )
        .await?;
    rows.iter()
        .map(|row| {
            Ok(ApiKeyView {
                id: text_at(row, 0)?,
                name: text_at(row, 1)?,
                prefix: text_at(row, 2)?,
                collection_id: opt_text_at(row, 3)?,
                role: role_at(row, 4)?,
                created_at: text_at(row, 5)?,
            })
        })
        .collect()
}

/// Finds a key whose owner is still enabled.
pub async fn find_api_key(conn: &Conn, key_hash: &str) -> anyhow::Result<Option<ApiKeyRecord>> {
    let rows = conn
        .query(
            "SELECT k.user_id, k.collection_id, u.email, u.handle, u.role, k.role
             FROM api_keys k JOIN users u ON u.id = k.user_id
             WHERE k.key_hash = ?1 AND u.disabled_at IS NULL",
            &[text(key_hash)],
        )
        .await?;
    optional_row(rows, |row| {
        Ok(ApiKeyRecord {
            user_id: text_at(row, 0)?,
            collection_id: opt_text_at(row, 1)?,
            email: text_at(row, 2)?,
            handle: text_at(row, 3)?,
            user_role: role_at(row, 4)?,
            key_role: role_at(row, 5)?,
        })
    })
}

pub async fn delete_api_key(conn: &Conn, user_id: &str, id: &str) -> anyhow::Result<bool> {
    let changed = conn
        .execute(
            "DELETE FROM api_keys WHERE id = ?1 AND user_id = ?2",
            &[text(id), text(user_id)],
        )
        .await?;
    Ok(changed > 0)
}

/// `created_by` is recorded for audit only; collections belong to the workspace.
pub async fn insert_collection(
    conn: &Conn,
    id: &str,
    created_by: &str,
    name: &str,
    visibility: Visibility,
    blueprint: &Blueprint,
    created_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO collections (id, owner_id, name, visibility, blueprint, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        &[
            text(id),
            text(created_by),
            text(name),
            text(visibility_str(visibility)),
            text(&serde_json::to_string(blueprint)?),
            text(created_at),
            text(created_at),
        ],
    )
    .await?;
    crate::rows::create(conn, id, blueprint).await?;
    Ok(())
}

pub async fn update_collection(
    conn: &Conn,
    id: &str,
    name: &str,
    visibility: Visibility,
    blueprint: &Blueprint,
    updated_at: &str,
) -> anyhow::Result<()> {
    let previous = find_collection(conn, id).await?;
    conn.execute(
        "UPDATE collections
         SET name = ?1, visibility = ?2, blueprint = ?3, updated_at = ?4
         WHERE id = ?5",
        &[
            text(name),
            text(visibility_str(visibility)),
            text(&serde_json::to_string(blueprint)?),
            text(updated_at),
            text(id),
        ],
    )
    .await?;
    conn.changed(id);
    // Labels, help texts and collection settings don't change stored values.
    if previous.is_none_or(|old| crate::rows::shape_changed(&old.blueprint, blueprint)) {
        crate::records::sync_collection_cells(conn, id, blueprint).await?;
    }
    Ok(())
}

pub async fn delete_collection(conn: &Conn, id: &str) -> anyhow::Result<()> {
    conn.execute("DELETE FROM collections WHERE id = ?1", &[text(id)])
        .await?;
    crate::rows::drop(conn, id).await?;
    conn.changed(id);
    Ok(())
}

pub async fn list_collections(
    conn: &Conn,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<CollectionView>> {
    let rows = conn
        .query(
            "SELECT id, name, visibility, blueprint, created_at, updated_at
             FROM collections
             ORDER BY created_at DESC, id DESC
             LIMIT ?1 OFFSET ?2",
            &[
                SqlValue::Integer(limit as i64),
                SqlValue::Integer(offset as i64),
            ],
        )
        .await?;
    rows.iter().map(collection_from_row).collect()
}

pub async fn find_collection(conn: &Conn, id: &str) -> anyhow::Result<Option<CollectionView>> {
    let rows = conn
        .query(
            "SELECT id, name, visibility, blueprint, created_at, updated_at
             FROM collections WHERE id = ?1",
            &[text(id)],
        )
        .await?;
    optional_row(rows, collection_from_row)
}

pub async fn find_collection_by_name(
    conn: &Conn,
    name: &str,
) -> anyhow::Result<Option<CollectionView>> {
    let rows = conn
        .query(
            "SELECT id, name, visibility, blueprint, created_at, updated_at
             FROM collections WHERE name = ?1",
            &[text(name)],
        )
        .await?;
    optional_row(rows, collection_from_row)
}

pub async fn list_documents(
    conn: &Conn,
    collection_id: &str,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<DocumentView>> {
    let rows = conn
        .query(
            "SELECT id, body, created_at, updated_at FROM documents
             WHERE collection_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2 OFFSET ?3",
            &[
                text(collection_id),
                SqlValue::Integer(limit as i64),
                SqlValue::Integer(offset as i64),
            ],
        )
        .await?;
    rows.iter().map(document_from_row).collect()
}

pub async fn list_document_bodies(
    conn: &Conn,
    collection_id: &str,
) -> anyhow::Result<Vec<(String, serde_json::Value)>> {
    let rows = conn
        .query(
            "SELECT id, body FROM documents WHERE collection_id = ?1",
            &[text(collection_id)],
        )
        .await?;
    rows.iter()
        .map(|row| {
            let id = text_at(row, 0)?;
            let body = serde_json::from_str(&text_at(row, 1)?)?;
            Ok((id, body))
        })
        .collect()
}

pub async fn find_document(
    conn: &Conn,
    collection_id: &str,
    id: &str,
) -> anyhow::Result<Option<DocumentView>> {
    let rows = conn
        .query(
            "SELECT id, body, created_at, updated_at FROM documents
             WHERE id = ?1 AND collection_id = ?2",
            &[text(id), text(collection_id)],
        )
        .await?;
    optional_row(rows, document_from_row)
}

pub async fn document_exists(conn: &Conn, collection_id: &str, id: &str) -> anyhow::Result<bool> {
    let rows = conn
        .query(
            "SELECT id FROM documents WHERE id = ?1 AND collection_id = ?2",
            &[text(id), text(collection_id)],
        )
        .await?;
    Ok(!rows.is_empty())
}

pub async fn insert_document(
    conn: &Conn,
    id: &str,
    collection_id: &str,
    body: &serde_json::Value,
    created_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO documents (id, collection_id, body, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        &[
            text(id),
            text(collection_id),
            text(&serde_json::to_string(body)?),
            text(created_at),
            text(created_at),
        ],
    )
    .await?;
    conn.changed(collection_id);
    crate::records::sync_stored_document(conn, collection_id, id, body).await?;
    crate::history::snapshot(conn, collection_id, id, "create").await?;
    Ok(())
}

pub async fn update_document(
    conn: &Conn,
    collection_id: &str,
    id: &str,
    body: &serde_json::Value,
    updated_at: &str,
) -> anyhow::Result<()> {
    update_document_as(conn, collection_id, id, body, updated_at, "update").await
}

/// Like `update_document`, recording `action` in the record's history.
pub async fn update_document_as(
    conn: &Conn,
    collection_id: &str,
    id: &str,
    body: &serde_json::Value,
    updated_at: &str,
    action: &str,
) -> anyhow::Result<()> {
    let previous = find_document(conn, collection_id, id).await?;
    conn.execute(
        "UPDATE documents SET body = ?1, updated_at = ?2 WHERE id = ?3 AND collection_id = ?4",
        &[
            text(&serde_json::to_string(body)?),
            text(updated_at),
            text(id),
            text(collection_id),
        ],
    )
    .await?;
    conn.changed(collection_id);
    crate::records::sync_stored_document(conn, collection_id, id, body).await?;
    if previous.as_ref().is_some_and(|old| old.body != *body) {
        crate::history::snapshot(conn, collection_id, id, action).await?;
    }
    Ok(())
}

pub async fn delete_document(conn: &Conn, collection_id: &str, id: &str) -> anyhow::Result<bool> {
    crate::history::snapshot(conn, collection_id, id, "delete").await?;
    let changed = conn
        .execute(
            "DELETE FROM documents WHERE id = ?1 AND collection_id = ?2",
            &[text(id), text(collection_id)],
        )
        .await?;
    crate::rows::remove(conn, collection_id, id).await?;
    conn.changed(collection_id);
    Ok(changed > 0)
}

/// An uploaded file's metadata, as stored in `assets`.
pub struct NewAsset<'a> {
    pub id: &'a str,
    pub uploaded_by: &'a str,
    pub collection_id: &'a str,
    pub filename: &'a str,
    pub content_type: &'a str,
    pub byte_size: i64,
    pub storage_key: &'a str,
    pub created_at: &'a str,
}

pub async fn insert_asset(conn: &Conn, asset: &NewAsset<'_>) -> anyhow::Result<()> {
    let NewAsset {
        id,
        uploaded_by,
        collection_id,
        filename,
        content_type,
        byte_size,
        storage_key,
        created_at,
    } = *asset;
    conn.execute(
        "INSERT INTO assets
         (id, owner_id, collection_id, filename, content_type, byte_size, storage_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        &[
            text(id),
            text(uploaded_by),
            text(collection_id),
            text(filename),
            text(content_type),
            SqlValue::Integer(byte_size),
            text(storage_key),
            text(created_at),
        ],
    )
    .await?;
    Ok(())
}

pub async fn list_assets(
    conn: &Conn,
    collection_id: &str,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<AssetView>> {
    let rows = conn
        .query(
            "SELECT id, filename, content_type, byte_size, storage_key, created_at
             FROM assets WHERE collection_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2 OFFSET ?3",
            &[
                text(collection_id),
                SqlValue::Integer(limit as i64),
                SqlValue::Integer(offset as i64),
            ],
        )
        .await?;
    rows.iter()
        .map(|row| Ok(asset_from_row(row)?.view))
        .collect()
}

pub async fn find_asset(
    conn: &Conn,
    collection_id: &str,
    id: &str,
) -> anyhow::Result<Option<AssetRecord>> {
    let rows = conn
        .query(
            "SELECT id, filename, content_type, byte_size, storage_key, created_at
             FROM assets WHERE id = ?1 AND collection_id = ?2",
            &[text(id), text(collection_id)],
        )
        .await?;
    optional_row(rows, asset_from_row)
}

pub async fn asset_exists(conn: &Conn, collection_id: &str, id: &str) -> anyhow::Result<bool> {
    let rows = conn
        .query(
            "SELECT id FROM assets WHERE id = ?1 AND collection_id = ?2",
            &[text(id), text(collection_id)],
        )
        .await?;
    Ok(!rows.is_empty())
}

pub async fn delete_asset(conn: &Conn, collection_id: &str, id: &str) -> anyhow::Result<bool> {
    let changed = conn
        .execute(
            "DELETE FROM assets WHERE id = ?1 AND collection_id = ?2",
            &[text(id), text(collection_id)],
        )
        .await?;
    Ok(changed > 0)
}

pub async fn asset_keys(conn: &Conn, collection_id: &str) -> anyhow::Result<Vec<String>> {
    let rows = conn
        .query(
            "SELECT storage_key FROM assets WHERE collection_id = ?1",
            &[text(collection_id)],
        )
        .await?;
    rows.iter().map(|row| text_at(row, 0)).collect()
}

fn user_from_row(row: &SqlRow) -> anyhow::Result<UserRecord> {
    Ok(UserRecord {
        id: text_at(row, 0)?,
        handle: text_at(row, 1)?,
        email: text_at(row, 2)?,
        password_hash: text_at(row, 3)?,
        role: role_at(row, 4)?,
        disabled: opt_text_at(row, 5)?.is_some(),
    })
}

fn role_at(row: &SqlRow, index: usize) -> anyhow::Result<Role> {
    let value = text_at(row, index)?;
    Role::parse(&value).ok_or_else(|| anyhow::anyhow!("unknown role {value:?}"))
}

fn purpose_str(purpose: LinkPurpose) -> &'static str {
    match purpose {
        LinkPurpose::Invite => "invite",
        LinkPurpose::Reset => "reset",
    }
}

fn purpose_at(row: &SqlRow, index: usize) -> anyhow::Result<LinkPurpose> {
    match text_at(row, index)?.as_str() {
        "invite" => Ok(LinkPurpose::Invite),
        "reset" => Ok(LinkPurpose::Reset),
        other => anyhow::bail!("unknown link purpose {other:?}"),
    }
}

fn link_view_from_row(row: &SqlRow) -> anyhow::Result<InviteView> {
    Ok(InviteView {
        id: text_at(row, 0)?,
        purpose: purpose_at(row, 1)?,
        email: text_at(row, 2)?,
        role: role_at(row, 3)?,
        created_at: text_at(row, 4)?,
        expires_at: text_at(row, 5)?,
    })
}

fn collection_from_row(row: &SqlRow) -> anyhow::Result<CollectionView> {
    Ok(CollectionView {
        id: text_at(row, 0)?,
        name: text_at(row, 1)?,
        visibility: parse_visibility(&text_at(row, 2)?)?,
        blueprint: serde_json::from_str(&text_at(row, 3)?)?,
        created_at: text_at(row, 4)?,
        updated_at: text_at(row, 5)?,
    })
}

pub(crate) fn document_from_row(row: &SqlRow) -> anyhow::Result<DocumentView> {
    Ok(DocumentView {
        calculation_errors: Default::default(),
        id: text_at(row, 0)?,
        body: serde_json::from_str(&text_at(row, 1)?)?,
        created_at: text_at(row, 2)?,
        updated_at: text_at(row, 3)?,
    })
}

fn asset_from_row(row: &SqlRow) -> anyhow::Result<AssetRecord> {
    let byte_size = int_at(row, 3)?;
    if byte_size < 0 {
        anyhow::bail!("negative asset size");
    }
    Ok(AssetRecord {
        view: AssetView {
            id: text_at(row, 0)?,
            filename: text_at(row, 1)?,
            content_type: text_at(row, 2)?,
            byte_size: byte_size as u64,
            created_at: text_at(row, 5)?,
        },
        storage_key: text_at(row, 4)?,
    })
}

fn optional_row<T>(
    rows: Vec<SqlRow>,
    map: impl Fn(&SqlRow) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    rows.first().map(map).transpose()
}

fn text(value: &str) -> SqlValue {
    SqlValue::Text(value.to_string())
}

fn visibility_str(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "public",
        Visibility::Private => "private",
    }
}

fn parse_visibility(value: &str) -> anyhow::Result<Visibility> {
    match value {
        "public" => Ok(Visibility::Public),
        "private" => Ok(Visibility::Private),
        _ => anyhow::bail!("invalid visibility {value}"),
    }
}
