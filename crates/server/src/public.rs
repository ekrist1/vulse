use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use vulse_model::{DocumentView, Page, Visibility};

use crate::content::{PageQuery, page_query};
use crate::error::AppError;
use crate::state::AppState;
use crate::store;

#[derive(Debug, Deserialize)]
pub struct PublicCollectionPath {
    pub collection: String,
}

#[derive(Debug, Deserialize)]
pub struct PublicDocumentPath {
    pub collection: String,
    pub doc_id: String,
}

#[derive(Debug, Deserialize)]
pub struct PublicAssetPath {
    pub collection: String,
    pub asset_id: String,
}

pub async fn list_documents(
    State(state): State<AppState>,
    Path(path): Path<PublicCollectionPath>,
    Query(query): Query<PageQuery>,
) -> Response {
    public_response(list_documents_inner(state, path, query).await)
}

pub async fn get_document(
    State(state): State<AppState>,
    Path(path): Path<PublicDocumentPath>,
) -> Response {
    public_response(get_document_inner(state, path).await)
}

pub async fn get_asset(
    State(state): State<AppState>,
    Path(path): Path<PublicAssetPath>,
) -> Response {
    match get_asset_inner(state, path).await {
        Ok(response) => response,
        Err(err) => with_public_cors(err.into_response()),
    }
}

/// Grouped totals of a public collection. Uses the same request as the
/// authenticated summary endpoint.
pub async fn summary(
    State(state): State<AppState>,
    Path(path): Path<PublicCollectionPath>,
    Json(request): Json<vulse_model::SummaryRequest>,
) -> Response {
    public_response(summary_inner(state, path, request).await)
}

async fn summary_inner(
    state: AppState,
    path: PublicCollectionPath,
    request: vulse_model::SummaryRequest,
) -> Result<Json<vulse_model::SummaryResult>, AppError> {
    let conn = state.db.connect().await?;
    let collection = public_collection(&conn, &path.collection).await?;
    Ok(Json(
        crate::summary::run_summary(
            &conn,
            &collection,
            &request,
            crate::summary::SummaryAccess::Public,
        )
        .await?,
    ))
}

pub async fn options() -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS"),
            (header::ACCESS_CONTROL_ALLOW_HEADERS, "*"),
            (header::ACCESS_CONTROL_MAX_AGE, "86400"),
        ],
    )
        .into_response()
}

async fn list_documents_inner(
    state: AppState,
    path: PublicCollectionPath,
    query: PageQuery,
) -> Result<Json<Page<DocumentView>>, AppError> {
    let (limit, offset) = page_query(&query)?;
    let conn = state.db.connect().await?;
    let collection = public_collection(&conn, &path.collection).await?;
    let mut items = store::list_documents(&conn, &collection.id, limit, offset).await?;
    let lookups = crate::lookups::load(&conn, &collection.blueprint).await?;
    for document in &mut items {
        crate::content::materialize(&collection.blueprint, &lookups, document);
    }
    Ok(Json(Page {
        items,
        limit,
        offset,
    }))
}

async fn get_document_inner(
    state: AppState,
    path: PublicDocumentPath,
) -> Result<Json<DocumentView>, AppError> {
    let conn = state.db.connect().await?;
    let collection = public_collection(&conn, &path.collection).await?;
    let mut document = store::find_document(&conn, &collection.id, &path.doc_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let lookups = crate::lookups::load(&conn, &collection.blueprint).await?;
    crate::content::materialize(&collection.blueprint, &lookups, &mut document);
    Ok(Json(document))
}

async fn get_asset_inner(state: AppState, path: PublicAssetPath) -> Result<Response, AppError> {
    let conn = state.db.connect().await?;
    let collection = public_collection(&conn, &path.collection).await?;
    let asset = store::find_asset(&conn, &collection.id, &path.asset_id)
        .await?
        .ok_or(AppError::NotFound)?;
    crate::assets::serve_file(&state.asset_dir, &asset, true).await
}

async fn public_collection(
    conn: &crate::db::Conn,
    name: &str,
) -> Result<vulse_model::CollectionView, AppError> {
    let name = name.trim().to_ascii_lowercase();
    let Some(collection) = store::find_collection_by_name(conn, &name).await? else {
        return Err(AppError::NotFound);
    };
    if collection.visibility != Visibility::Public {
        return Err(AppError::NotFound);
    }
    Ok(collection)
}

fn public_response(result: Result<impl IntoResponse, AppError>) -> Response {
    let response = match result {
        Ok(body) => body.into_response(),
        Err(err) => err.into_response(),
    };
    with_public_cors(response)
}

fn with_public_cors(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        axum::http::HeaderValue::from_static("*"),
    );
    response
}
