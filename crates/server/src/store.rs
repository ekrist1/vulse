use vulse_model::{ApiKeyView, AssetView, Blueprint, CollectionView, DocumentView, Visibility};

use crate::db::{Conn, SqlRow, SqlValue, int_at, opt_text_at, text_at};

pub struct UserRecord {
    pub id: String,
    pub handle: String,
    pub email: String,
    pub password_hash: String,
}

pub struct SessionRecord {
    pub user_id: String,
    pub email: String,
    pub handle: String,
    pub expires_at: String,
}

pub struct ApiKeyRecord {
    pub user_id: String,
    pub email: String,
    pub handle: String,
    pub collection_id: Option<String>,
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
    created_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO users (id, handle, email, password_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        &[
            text(id),
            text(handle),
            text(email),
            text(password_hash),
            text(created_at),
        ],
    )
    .await?;
    Ok(())
}

pub async fn find_user_by_email(conn: &Conn, email: &str) -> anyhow::Result<Option<UserRecord>> {
    let rows = conn
        .query(
            "SELECT id, handle, email, password_hash FROM users WHERE email = ?1",
            &[text(email)],
        )
        .await?;
    optional_row(rows, user_from_row)
}

pub async fn find_user_by_handle(conn: &Conn, handle: &str) -> anyhow::Result<Option<UserRecord>> {
    let rows = conn
        .query(
            "SELECT id, handle, email, password_hash FROM users WHERE handle = ?1",
            &[text(handle)],
        )
        .await?;
    optional_row(rows, user_from_row)
}

pub async fn insert_session(
    conn: &Conn,
    token: &str,
    user_id: &str,
    expires_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO sessions (token, user_id, expires_at) VALUES (?1, ?2, ?3)",
        &[text(token), text(user_id), text(expires_at)],
    )
    .await?;
    Ok(())
}

pub async fn find_session(conn: &Conn, token: &str) -> anyhow::Result<Option<SessionRecord>> {
    let rows = conn
        .query(
            "SELECT s.user_id, s.expires_at, u.email, u.handle
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token = ?1",
            &[text(token)],
        )
        .await?;
    optional_row(rows, |row| {
        Ok(SessionRecord {
            user_id: text_at(row, 0)?,
            expires_at: text_at(row, 1)?,
            email: text_at(row, 2)?,
            handle: text_at(row, 3)?,
        })
    })
}

pub async fn delete_session(conn: &Conn, token: &str) -> anyhow::Result<()> {
    conn.execute("DELETE FROM sessions WHERE token = ?1", &[text(token)])
        .await?;
    Ok(())
}

pub async fn insert_api_key(
    conn: &Conn,
    id: &str,
    user_id: &str,
    name: &str,
    prefix: &str,
    key_hash: &str,
    collection_id: Option<&str>,
    created_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO api_keys (id, user_id, name, key_prefix, key_hash, collection_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        &[
            text(id),
            text(user_id),
            text(name),
            text(prefix),
            text(key_hash),
            collection_id.map(text).unwrap_or(SqlValue::Null),
            text(created_at),
        ],
    )
    .await?;
    Ok(())
}

pub async fn list_api_keys(conn: &Conn, user_id: &str) -> anyhow::Result<Vec<ApiKeyView>> {
    let rows = conn
        .query(
            "SELECT id, name, key_prefix, collection_id, created_at
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
                created_at: text_at(row, 4)?,
            })
        })
        .collect()
}

pub async fn find_api_key(conn: &Conn, key_hash: &str) -> anyhow::Result<Option<ApiKeyRecord>> {
    let rows = conn
        .query(
            "SELECT k.user_id, k.collection_id, u.email, u.handle
             FROM api_keys k JOIN users u ON u.id = k.user_id
             WHERE k.key_hash = ?1",
            &[text(key_hash)],
        )
        .await?;
    optional_row(rows, |row| {
        Ok(ApiKeyRecord {
            user_id: text_at(row, 0)?,
            collection_id: opt_text_at(row, 1)?,
            email: text_at(row, 2)?,
            handle: text_at(row, 3)?,
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

pub async fn insert_collection(
    conn: &Conn,
    id: &str,
    owner_id: &str,
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
            text(owner_id),
            text(name),
            text(visibility_str(visibility)),
            text(&serde_json::to_string(blueprint)?),
            text(created_at),
            text(created_at),
        ],
    )
    .await?;
    Ok(())
}

pub async fn update_collection(
    conn: &Conn,
    owner_id: &str,
    id: &str,
    name: &str,
    visibility: Visibility,
    blueprint: &Blueprint,
    updated_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE collections
         SET name = ?1, visibility = ?2, blueprint = ?3, updated_at = ?4
         WHERE id = ?5 AND owner_id = ?6",
        &[
            text(name),
            text(visibility_str(visibility)),
            text(&serde_json::to_string(blueprint)?),
            text(updated_at),
            text(id),
            text(owner_id),
        ],
    )
    .await?;
    crate::records::sync_collection_cells(conn, id, blueprint).await?;
    Ok(())
}

pub async fn delete_collection(conn: &Conn, owner_id: &str, id: &str) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM collections WHERE id = ?1 AND owner_id = ?2",
        &[text(id), text(owner_id)],
    )
    .await?;
    Ok(())
}

pub async fn list_collections(
    conn: &Conn,
    owner_id: &str,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<CollectionView>> {
    let rows = conn
        .query(
            "SELECT id, name, visibility, blueprint, created_at, updated_at
             FROM collections WHERE owner_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT ?2 OFFSET ?3",
            &[
                text(owner_id),
                SqlValue::Integer(limit as i64),
                SqlValue::Integer(offset as i64),
            ],
        )
        .await?;
    rows.iter().map(collection_from_row).collect()
}

pub async fn find_collection(
    conn: &Conn,
    owner_id: &str,
    id: &str,
) -> anyhow::Result<Option<CollectionView>> {
    let rows = conn
        .query(
            "SELECT id, name, visibility, blueprint, created_at, updated_at
             FROM collections WHERE id = ?1 AND owner_id = ?2",
            &[text(id), text(owner_id)],
        )
        .await?;
    optional_row(rows, |row| collection_from_row(row))
}

pub async fn find_collection_by_name(
    conn: &Conn,
    owner_id: &str,
    name: &str,
) -> anyhow::Result<Option<CollectionView>> {
    let rows = conn
        .query(
            "SELECT id, name, visibility, blueprint, created_at, updated_at
             FROM collections WHERE owner_id = ?1 AND name = ?2",
            &[text(owner_id), text(name)],
        )
        .await?;
    optional_row(rows, |row| collection_from_row(row))
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
    optional_row(rows, |row| document_from_row(row))
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
    crate::records::sync_stored_document(conn, collection_id, id, body).await?;
    if previous.as_ref().is_some_and(|old| old.body != *body) {
        crate::history::snapshot(conn, collection_id, id, "update").await?;
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
    Ok(changed > 0)
}

pub async fn insert_asset(
    conn: &Conn,
    id: &str,
    owner_id: &str,
    collection_id: &str,
    filename: &str,
    content_type: &str,
    byte_size: i64,
    storage_key: &str,
    created_at: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO assets
         (id, owner_id, collection_id, filename, content_type, byte_size, storage_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        &[
            text(id),
            text(owner_id),
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
