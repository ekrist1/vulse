use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use vulse_model::{
    Blueprint, CollectionView, CreateCollection, DocumentView, DocumentWrite, Page,
    UpdateCollection, check_collection_name, page_bounds, reference_collections,
    validate_blueprint, validate_document,
};

use crate::auth::{AccountOnly, AuthUser, Principal, require_collection};
use crate::db::Conn;
use crate::error::{AppError, unique_message};
use crate::state::AppState;
use crate::store;
use crate::timeutil;

#[derive(Debug, Deserialize)]
pub struct PageQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

pub fn page_query(query: &PageQuery) -> Result<(u32, u32), AppError> {
    page_bounds(query.limit, query.offset).map_err(AppError::bad)
}

pub async fn list_collections(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<CollectionView>>, AppError> {
    let (limit, offset) = page_query(&query)?;
    let conn = state.db.connect().await?;
    let items = if let Some(scope) = &principal.collection_scope {
        store::find_collection(&conn, &principal.user_id, scope)
            .await?
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect()
    } else {
        store::list_collections(&conn, &principal.user_id, limit, offset).await?
    };
    Ok(Json(Page {
        items,
        limit,
        offset,
    }))
}

pub async fn create_collection(
    State(state): State<AppState>,
    AccountOnly(principal): AccountOnly,
    Json(body): Json<CreateCollection>,
) -> Result<(StatusCode, Json<CollectionView>), AppError> {
    let name = body.name.trim().to_ascii_lowercase();
    check_collection_name(&name).map_err(AppError::bad)?;
    prepare_blueprint(&body.blueprint, &name)?;
    let conn = state.db.connect().await?;
    let id = crate::auth::new_id();
    let created_at = timeutil::now_rfc3339();
    conn.with_app_tx(async |conn| {
        ensure_targets(conn, &principal.user_id, &body.blueprint, &name).await?;
        store::insert_collection(
            conn,
            &id,
            &principal.user_id,
            &name,
            body.visibility,
            &body.blueprint,
            &created_at,
        )
        .await
        .map_err(|e| unique_message(e, "a collection with that name already exists"))?;
        Ok(())
    })
    .await?;
    let collection = store::find_collection(&conn, &principal.user_id, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("created collection disappeared"))?;
    Ok((StatusCode::CREATED, Json(collection)))
}

pub async fn get_collection(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<CollectionView>, AppError> {
    let conn = state.db.connect().await?;
    let collection = owned_collection(&conn, &principal, &id).await?;
    Ok(Json(collection))
}

pub async fn update_collection(
    State(state): State<AppState>,
    AccountOnly(principal): AccountOnly,
    Path(id): Path<String>,
    Json(body): Json<UpdateCollection>,
) -> Result<Json<CollectionView>, AppError> {
    let conn = state.db.connect().await?;
    let collection = conn
        .with_app_tx(async |conn| {
            let current = owned_collection(conn, &principal, &id).await?;
            if body
                .expected_updated_at
                .as_ref()
                .is_some_and(|expected| *expected != current.updated_at)
            {
                return Err(AppError::conflict(
                    "Collection fields changed since you loaded them. Reload before saving.",
                ));
            }
            let name = body
                .name
                .as_ref()
                .map(|name| name.trim().to_ascii_lowercase())
                .unwrap_or(current.name.clone());
            check_collection_name(&name).map_err(AppError::bad)?;
            let visibility = body.visibility.unwrap_or(current.visibility);
            let mut blueprint = body.blueprint.clone().unwrap_or(current.blueprint.clone());
            retarget_self(&mut blueprint, &current.name, &name);
            prepare_blueprint(&blueprint, &name)?;
            ensure_targets(conn, &principal.user_id, &blueprint, &name).await?;
            let mut failing = Vec::new();
            for (doc, body) in store::list_document_bodies(conn, &id).await? {
                match check_body(conn, &principal.user_id, &id, &name, &blueprint, &body).await {
                    Ok(()) => {}
                    Err(AppError::BadRequest(_)) => failing.push(doc),
                    Err(err) => return Err(err),
                }
            }
            if !failing.is_empty() {
                return Err(AppError::conflict_docs(
                    "blueprint would invalidate existing documents",
                    failing,
                ));
            }
            store::update_collection(
                conn,
                &principal.user_id,
                &id,
                &name,
                visibility,
                &blueprint,
                &timeutil::now_rfc3339(),
            )
            .await
            .map_err(|err| unique_message(err, "a collection with that name already exists"))?;
            if name != current.name {
                for mut related in
                    crate::relationships::owner_collections(conn, &principal.user_id).await?
                {
                    if related.id == id {
                        continue;
                    }
                    let original = related.blueprint.clone();
                    retarget_self(&mut related.blueprint, &current.name, &name);
                    if original != related.blueprint {
                        store::update_collection(
                            conn,
                            &principal.user_id,
                            &related.id,
                            &related.name,
                            related.visibility,
                            &related.blueprint,
                            &timeutil::now_rfc3339(),
                        )
                        .await?;
                    }
                }
            }
            store::find_collection(conn, &principal.user_id, &id)
                .await?
                .ok_or(AppError::NotFound)
        })
        .await?;
    Ok(Json(collection))
}

pub async fn delete_collection(
    State(state): State<AppState>,
    AccountOnly(principal): AccountOnly,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let conn = state.db.connect().await?;
    let keys=conn.with_app_tx(async |conn| {
        let current=owned_collection(conn,&principal,&id).await?;
        for related in crate::relationships::owner_collections(conn,&principal.user_id).await?{
            if related.id!=id && reference_collections(&related.blueprint).contains(&current.name.as_str()){
                return Err(AppError::conflict(format!("Collection '{}' has fields referencing this collection. Remove those fields first.",related.name)));
            }
        }
        let keys=store::asset_keys(conn,&id).await?;
        store::delete_collection(conn,&principal.user_id,&id).await?;
        Ok(keys)
    }).await?;
    for key in keys {
        if let Some(path) = crate::assets::asset_path(&state.asset_dir, &key) {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_documents(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<DocumentView>>, AppError> {
    let (limit, offset) = page_query(&query)?;
    let conn = state.db.connect().await?;
    let collection = owned_collection(&conn, &principal, &id).await?;
    let mut items = store::list_documents(&conn, &id, limit, offset).await?;
    for document in &mut items {
        materialize(&collection.blueprint, document);
    }
    Ok(Json(Page {
        items,
        limit,
        offset,
    }))
}

pub async fn create_document(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path(id): Path<String>,
    Json(body): Json<DocumentWrite>,
) -> Result<(StatusCode, Json<DocumentView>), AppError> {
    let conn = state.db.connect().await?;
    let document = conn
        .with_app_tx(async |conn| {
            let collection = owned_collection(conn, &principal, &id).await?;
            let source = vulse_model::source_document(&collection.blueprint, &body.body);
            check_body(
                conn,
                &principal.user_id,
                &id,
                &collection.name,
                &collection.blueprint,
                &source,
            )
            .await?;
            let document_id = crate::auth::new_id();
            store::insert_document(conn, &document_id, &id, &source, &timeutil::now_rfc3339())
                .await?;
            let mut document = store::find_document(conn, &id, &document_id)
                .await?
                .ok_or(AppError::NotFound)?;
            materialize(&collection.blueprint, &mut document);
            Ok(document)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(document)))
}

pub async fn get_document(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path((id, doc_id)): Path<(String, String)>,
) -> Result<Json<DocumentView>, AppError> {
    let conn = state.db.connect().await?;
    let collection = owned_collection(&conn, &principal, &id).await?;
    let mut document = store::find_document(&conn, &collection.id, &doc_id)
        .await?
        .ok_or(AppError::NotFound)?;
    materialize(&collection.blueprint, &mut document);
    Ok(Json(document))
}

pub async fn update_document(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path((id, doc_id)): Path<(String, String)>,
    Json(body): Json<DocumentWrite>,
) -> Result<Json<DocumentView>, AppError> {
    let conn = state.db.connect().await?;
    let document = conn
        .with_app_tx(async |conn| {
            let collection = owned_collection(conn, &principal, &id).await?;
            let current = store::find_document(conn, &id, &doc_id)
                .await?
                .ok_or(AppError::NotFound)?;
            crate::mutations::check_version(&current, body.expected_updated_at.as_deref())?;
            let source = vulse_model::source_document(&collection.blueprint, &body.body);
            check_body(
                conn,
                &principal.user_id,
                &id,
                &collection.name,
                &collection.blueprint,
                &source,
            )
            .await?;
            store::update_document(conn, &id, &doc_id, &source, &timeutil::now_rfc3339()).await?;
            let mut document = store::find_document(conn, &id, &doc_id)
                .await?
                .ok_or(AppError::NotFound)?;
            materialize(&collection.blueprint, &mut document);
            Ok(document)
        })
        .await?;
    Ok(Json(document))
}

pub async fn delete_document(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path((id, doc_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    crate::mutations::run_bulk(
        &state,
        &principal,
        &id,
        vulse_model::BulkWrite {
            action: "delete".into(),
            ids: vec![doc_id],
            patch: serde_json::Value::Null,
            expected: Default::default(),
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn owned_collection(
    conn: &Conn,
    principal: &Principal,
    id: &str,
) -> Result<CollectionView, AppError> {
    let collection = store::find_collection(conn, &principal.user_id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_collection(principal, &collection.id)?;
    Ok(collection)
}

pub(crate) async fn check_body(
    conn: &Conn,
    owner_id: &str,
    collection_id: &str,
    collection_name: &str,
    blueprint: &Blueprint,
    body: &serde_json::Value,
) -> Result<(), AppError> {
    let checked = validate_document(blueprint, body).map_err(AppError::bad)?;
    for asset_id in &checked.asset_ids {
        if !store::asset_exists(conn, collection_id, asset_id).await? {
            return Err(AppError::bad(format!("unknown asset {asset_id}")));
        }
    }
    for reference in checked.references {
        let target_id = if reference.collection == collection_name {
            collection_id.to_string()
        } else {
            store::find_collection_by_name(conn, owner_id, &reference.collection)
                .await?
                .ok_or_else(|| {
                    AppError::bad(format!("unknown collection '{}'", reference.collection))
                })?
                .id
        };
        if !store::document_exists(conn, &target_id, &reference.document_id).await? {
            return Err(AppError::bad(format!(
                "unknown document {}",
                reference.document_id
            )));
        }
    }
    Ok(())
}

fn prepare_blueprint(blueprint: &Blueprint, self_name: &str) -> Result<(), AppError> {
    validate_blueprint(blueprint).map_err(AppError::bad)?;
    for name in reference_collections(blueprint) {
        if name != self_name {
            check_collection_name(name).map_err(AppError::bad)?;
        }
    }
    Ok(())
}

async fn ensure_targets(
    conn: &Conn,
    owner_id: &str,
    blueprint: &Blueprint,
    self_name: &str,
) -> Result<(), AppError> {
    for name in reference_collections(blueprint) {
        if name == self_name {
            continue;
        }
        if store::find_collection_by_name(conn, owner_id, name)
            .await?
            .is_none()
        {
            return Err(AppError::bad(format!("unknown collection '{name}'")));
        }
    }
    Ok(())
}

fn retarget_self(blueprint: &mut Blueprint, old_name: &str, new_name: &str) {
    if old_name == new_name {
        return;
    }
    for field in &mut blueprint.fields {
        if field.collection.as_deref() == Some(old_name) {
            field.collection = Some(new_name.to_string());
        }
    }
}

pub(crate) fn materialize(blueprint: &Blueprint, document: &mut DocumentView) {
    let (body, errors) = vulse_model::calculate_document(blueprint, &document.body);
    document.body = body;
    document.calculation_errors = errors;
}
