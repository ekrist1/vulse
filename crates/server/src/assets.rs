use std::path::{Path, PathBuf};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Multipart, Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderValue};
use axum::response::{IntoResponse, Response};
use vulse_model::{AssetView, Page, is_uuid};

use crate::auth::{AuthUser, Editor};
use crate::content::{PageQuery, page_query, workspace_collection};
use crate::error::AppError;
use crate::state::AppState;
use crate::store;
use crate::timeutil;

pub const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

pub async fn list_assets(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<AssetView>>, AppError> {
    let (limit, offset) = page_query(&query)?;
    let conn = state.db.connect().await?;
    let collection = workspace_collection(&conn, &principal, &id).await?;
    let items = store::list_assets(&conn, &collection.id, limit, offset).await?;
    Ok(Json(Page {
        items,
        limit,
        offset,
    }))
}

pub async fn upload_asset(
    State(state): State<AppState>,
    Editor(principal): Editor,
    AxumPath(id): AxumPath<String>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<AssetView>), AppError> {
    let conn = state.db.connect().await?;
    let collection = workspace_collection(&conn, &principal, &id).await?;
    let mut file: Option<(String, String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| AppError::bad(err.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = safe_filename(field.file_name().unwrap_or("file"));
        let content_type = normalize_content_type(field.content_type().unwrap_or(""))
            .ok_or_else(|| AppError::bad("unsupported content type"))?;
        let data = field
            .bytes()
            .await
            .map_err(|err| AppError::bad(err.to_string()))?;
        if data.len() > MAX_UPLOAD_BYTES {
            return Err(AppError::PayloadTooLarge);
        }
        file = Some((filename, content_type, data.to_vec()));
        break;
    }
    let Some((filename, content_type, data)) = file else {
        return Err(AppError::bad("missing file"));
    };
    let asset_id = crate::auth::new_id();
    let storage_key = format!("{}/{}", principal.user_id, asset_id);
    let path = asset_path(&state.asset_dir, &storage_key).ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!("refusing to store an unsafe asset path"))
    })?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| AppError::Internal(err.into()))?;
    }
    tokio::fs::write(&path, &data)
        .await
        .map_err(|err| AppError::Internal(err.into()))?;
    if !path_inside(&state.asset_dir, &path) {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(AppError::Internal(anyhow::anyhow!(
            "asset path escaped the storage directory"
        )));
    }
    let created_at = timeutil::now_rfc3339();
    if let Err(err) = store::insert_asset(
        &conn,
        &asset_id,
        &principal.user_id,
        &collection.id,
        &filename,
        &content_type,
        data.len() as i64,
        &storage_key,
        &created_at,
    )
    .await
    {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(err.into());
    }
    let asset = store::find_asset(&conn, &collection.id, &asset_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("created asset disappeared"))?;
    Ok((StatusCode::CREATED, Json(asset.view)))
}

pub async fn get_asset(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    AxumPath((id, asset_id)): AxumPath<(String, String)>,
) -> Result<Json<AssetView>, AppError> {
    let conn = state.db.connect().await?;
    let collection = workspace_collection(&conn, &principal, &id).await?;
    let asset = store::find_asset(&conn, &collection.id, &asset_id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(asset.view))
}

pub async fn get_asset_content(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    AxumPath((id, asset_id)): AxumPath<(String, String)>,
) -> Result<Response, AppError> {
    let conn = state.db.connect().await?;
    let collection = workspace_collection(&conn, &principal, &id).await?;
    let asset = store::find_asset(&conn, &collection.id, &asset_id)
        .await?
        .ok_or(AppError::NotFound)?;
    serve_file(&state.asset_dir, &asset, false).await
}

pub async fn delete_asset(
    State(state): State<AppState>,
    Editor(principal): Editor,
    AxumPath((id, asset_id)): AxumPath<(String, String)>,
) -> Result<StatusCode, AppError> {
    let conn = state.db.connect().await?;
    let asset = conn
        .with_app_tx(async |conn| {
            let collection = workspace_collection(conn, &principal, &id).await?;
            let asset = store::find_asset(conn, &collection.id, &asset_id)
                .await?
                .ok_or(AppError::NotFound)?;
            crate::relationships::guard_delete(conn, &id, std::slice::from_ref(&asset_id), "asset")
                .await?;
            store::delete_asset(conn, &id, &asset_id).await?;
            Ok(asset)
        })
        .await?;
    if let Some(path) = asset_path(&state.asset_dir, &asset.storage_key) {
        let _ = tokio::fs::remove_file(path).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn serve_file(
    root: &Path,
    asset: &store::AssetRecord,
    public: bool,
) -> Result<Response, AppError> {
    let Some(path) = asset_path(root, &asset.storage_key) else {
        tracing::error!(key = %asset.storage_key, "rejecting unsafe asset key");
        return Err(AppError::NotFound);
    };
    if !path_inside(root, &path) {
        tracing::error!(key = %asset.storage_key, "asset path escaped storage");
        return Err(AppError::NotFound);
    }
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(AppError::NotFound),
        Err(err) => return Err(anyhow::anyhow!(err).into()),
    };
    let content_type = normalize_content_type(&asset.view.content_type)
        .ok_or_else(|| anyhow::anyhow!("stored content type is not servable"))?;
    Ok(file_response(
        bytes,
        &content_type,
        &asset.view.filename,
        public,
    ))
}

pub fn asset_path(root: &Path, storage_key: &str) -> Option<PathBuf> {
    let mut parts = storage_key.split('/');
    let owner = parts.next()?;
    let asset = parts.next()?;
    if parts.next().is_some() || !is_uuid(owner) || !is_uuid(asset) {
        return None;
    }
    Some(root.join(owner).join(asset))
}

pub fn normalize_content_type(raw: &str) -> Option<String> {
    let essence = raw.split(';').next()?.trim().to_ascii_lowercase();
    if essence.is_empty()
        || !essence
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '+' | '-' | '.'))
        || essence == "image/svg+xml"
    {
        return None;
    }
    if essence.starts_with("image/")
        || essence.starts_with("video/")
        || essence.starts_with("audio/")
        || essence == "application/pdf"
    {
        Some(essence)
    } else {
        None
    }
}

fn path_inside(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(root)
}

fn safe_filename(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_ascii() && !c.is_control() && *c != '"' && *c != '\\' && *c != '/')
        .take(200)
        .collect();
    if cleaned.is_empty() {
        "file".into()
    } else {
        cleaned
    }
}

fn file_response(bytes: Vec<u8>, content_type: &str, filename: &str, public: bool) -> Response {
    let ascii: String = filename
        .chars()
        .filter(|c| c.is_ascii_graphic() && *c != '"' && *c != '\\')
        .collect();
    let filename = if ascii.is_empty() { "file" } else { &ascii };
    let disposition = format!("inline; filename=\"{filename}\"");
    let mut response = (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(content_type)
                    .unwrap_or(HeaderValue::from_static("application/octet-stream")),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("sandbox"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&disposition).unwrap_or(HeaderValue::from_static("inline")),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static(if public {
                    "public, max-age=3600"
                } else {
                    "private, max-age=3600"
                }),
            ),
        ],
        Bytes::from(bytes),
    )
        .into_response();
    if public {
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_keys_cannot_escape_the_storage_directory() {
        let root = Path::new("/tmp/vulse-assets");
        let owner = "550e8400-e29b-41d4-a716-446655440000";
        let asset = "550e8400-e29b-41d4-a716-446655440001";
        let path = asset_path(root, &format!("{owner}/{asset}")).unwrap();
        assert_eq!(path, root.join(owner).join(asset));
        assert!(asset_path(root, "../etc/passwd").is_none());
        assert!(asset_path(root, "/etc/passwd").is_none());
        assert!(asset_path(root, &format!("{owner}/{asset}/extra")).is_none());
        assert!(asset_path(root, "not-a-uuid/not-a-uuid").is_none());
    }

    #[test]
    fn content_types_reject_svg_and_header_injection() {
        assert_eq!(
            normalize_content_type("image/png; charset=binary").as_deref(),
            Some("image/png")
        );
        assert!(normalize_content_type("image/svg+xml").is_none());
        assert!(normalize_content_type("image/png\r\nX-Evil: 1").is_none());
        assert!(normalize_content_type("text/html").is_none());
        assert_eq!(
            normalize_content_type("application/pdf").as_deref(),
            Some("application/pdf")
        );
    }
}
