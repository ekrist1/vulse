use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use std::collections::BTreeMap;

use vulse_model::{
    Blueprint, CollectionView, CreateCollection, DocumentView, DocumentWrite, FieldType, Page,
    UpdateCollection, check_collection_name, page_bounds, reference_collections,
    take_field_renames, take_option_renames, validate_blueprint, validate_document,
};

use crate::auth::{AuthUser, Editor, Principal, SchemaAdmin, require_collection};
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
    let items = visible_collections(&conn, &principal, limit, offset).await?;
    Ok(Json(Page {
        items,
        limit,
        offset,
    }))
}

/// The collections a principal may read: all of them, or the one its key is
/// limited to.
pub(crate) async fn visible_collections(
    conn: &Conn,
    principal: &Principal,
    limit: u32,
    offset: u32,
) -> Result<Vec<CollectionView>, AppError> {
    Ok(if let Some(scope) = &principal.collection_scope {
        store::find_collection(conn, scope)
            .await?
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect()
    } else {
        store::list_collections(conn, limit, offset).await?
    })
}

pub async fn create_collection(
    State(state): State<AppState>,
    SchemaAdmin(principal): SchemaAdmin,
    Json(body): Json<CreateCollection>,
) -> Result<(StatusCode, Json<CollectionView>), AppError> {
    let name = body.name.trim().to_ascii_lowercase();
    check_collection_name(&name).map_err(AppError::bad)?;
    prepare_blueprint(&body.blueprint, &name)?;
    let conn = state.db.connect().await?;
    let id = crate::auth::new_id();
    let created_at = timeutil::now_rfc3339();
    conn.with_app_tx(async |conn| {
        ensure_targets(conn, &body.blueprint, &name).await?;
        crate::lookups::check(conn, None, &name, &body.blueprint).await?;
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
    let collection = store::find_collection(&conn, &id)
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
    let collection = workspace_collection(&conn, &principal, &id).await?;
    Ok(Json(collection))
}

pub async fn update_collection(
    State(state): State<AppState>,
    SchemaAdmin(principal): SchemaAdmin,
    Path(id): Path<String>,
    Json(body): Json<UpdateCollection>,
) -> Result<Json<CollectionView>, AppError> {
    let conn = state.db.connect().await?;
    let collection = conn
        .with_app_tx(async |conn| {
            let current = workspace_collection(conn, &principal, &id).await?;
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
            // Renamed fields keep their values, formulas, title and views.
            let renames =
                take_field_renames(&mut blueprint, &current.blueprint).map_err(AppError::bad)?;
            // Renamed allowed values move in the records holding them.
            let value_renames = take_option_renames(&mut blueprint, &current.blueprint, &renames)
                .map_err(AppError::bad)?;
            retarget_self(&mut blueprint, &current.name, &name);
            crate::lookups::rename(&mut blueprint, &current.name, &name, &renames);
            prepare_blueprint(&blueprint, &name)?;
            ensure_targets(conn, &blueprint, &name).await?;
            let previous = crate::lookups::Previous {
                id: &id,
                name: &current.name,
                renames: &renames,
            };
            crate::lookups::check(conn, Some(previous), &name, &blueprint).await?;
            // Stored values of removed fields are only deleted on request.
            let removed: Vec<String> = current
                .blueprint
                .fields
                .iter()
                .filter(|old| old.kind != FieldType::Calculated)
                .filter(|old| !blueprint.fields.iter().any(|new| new.name == old.name))
                .filter(|old| !renames.contains_key(&old.name))
                .map(|old| old.name.clone())
                .collect();
            // Stored fields that keep their data (by name or rename) but change type.
            let retyped: Vec<(String, vulse_model::FieldDef, vulse_model::FieldDef)> = blueprint
                .fields
                .iter()
                .filter(|new| new.kind != FieldType::Calculated)
                .filter_map(|new| {
                    let origin = renames
                        .iter()
                        .find(|(_, to)| **to == new.name)
                        .map(|(from, _)| from.as_str())
                        .unwrap_or(new.name.as_str());
                    let old = current.blueprint.fields.iter().find(|f| f.name == origin)?;
                    (old.kind != FieldType::Calculated
                        && (old.kind != new.kind || old.items != new.items))
                        .then(|| (new.name.clone(), old.clone(), new.clone()))
                })
                .collect();
            let mut unconvertible = Vec::<(String, String)>::new();
            let mut unconvertible_docs = Vec::new();
            let mut holding = BTreeMap::<String, usize>::new();
            let mut holders = Vec::new();
            let mut stripped = Vec::new();
            let mut failing = Vec::new();
            let mut first_error = None;
            let mut found = Found::default();
            for (doc, mut source) in store::list_document_bodies(conn, &id).await? {
                if let Some(object) = source.as_object_mut() {
                    let mut renamed = false;
                    for (old, new) in &renames {
                        if let Some(value) = object.remove(old) {
                            object.insert(new.clone(), value);
                            renamed = true;
                        }
                    }
                    let mut values_renamed = false;
                    for (field, values) in &value_renames {
                        let next = object
                            .get(field)
                            .and_then(|value| value.as_str())
                            .and_then(|value| values.get(value));
                        if let Some(next) = next {
                            object.insert(field.clone(), serde_json::Value::String(next.clone()));
                            values_renamed = true;
                        }
                    }
                    let mut changed = false;
                    for field in &removed {
                        if let Some(value) = object.remove(field) {
                            changed = true;
                            if !value.is_null() {
                                *holding.entry(field.clone()).or_default() += 1;
                            }
                        }
                    }
                    let mut converted = false;
                    if body.convert_field_values {
                        for (name, old, new) in &retyped {
                            let Some(value) = object.get(name) else {
                                continue;
                            };
                            match crate::convert::convert_value(value, old, new) {
                                Ok(Some(next)) if next == *value => {}
                                Ok(Some(next)) => {
                                    object.insert(name.clone(), next);
                                    converted = true;
                                }
                                Ok(None) => {
                                    object.remove(name);
                                    converted = true;
                                }
                                Err(reason) => {
                                    if unconvertible.len() < 5 {
                                        let shown: String =
                                            vulse_model::display_value(value).chars().take(40).collect();
                                        unconvertible.push((name.clone(), format!("“{shown}” {reason}")));
                                    }
                                    unconvertible_docs.push(doc.clone());
                                }
                            }
                        }
                    }
                    if changed {
                        holders.push(doc.clone());
                    }
                    if changed || renamed || converted || values_renamed {
                        let action = if converted {
                            "fields converted"
                        } else if renamed {
                            "fields renamed"
                        } else if values_renamed {
                            "values renamed"
                        } else {
                            "fields removed"
                        };
                        stripped.push((doc.clone(), source.clone(), action));
                    }
                }
                match check_body_with(conn, &id, &name, &blueprint, &source, &mut found).await {
                    Ok(()) => {}
                    Err(AppError::BadRequest(message)) => {
                        first_error.get_or_insert(message);
                        failing.push(doc);
                    }
                    Err(err) => return Err(err),
                }
            }
            if !holding.is_empty() && !body.remove_field_data {
                let fields = holding
                    .iter()
                    .map(|(field, count)| {
                        format!("'{field}' ({count} record{})", if *count == 1 { "" } else { "s" })
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(AppError::conflict_docs(
                    format!(
                        "Removing fields deletes their stored values: {fields}. Confirm with remove_field_data to continue."
                    ),
                    holders,
                ));
            }
            if !unconvertible.is_empty() {
                unconvertible_docs.dedup();
                let examples = unconvertible
                    .iter()
                    .map(|(field, problem)| format!("{field}: {problem}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(AppError::conflict_docs(
                    format!(
                        "{} record{} cannot be converted. Fix or clear these values first: {examples}",
                        unconvertible_docs.len(),
                        if unconvertible_docs.len() == 1 { "" } else { "s" }
                    ),
                    unconvertible_docs,
                ));
            }
            if !failing.is_empty() {
                let hint = if !retyped.is_empty() && !body.convert_field_values {
                    format!(
                        " To convert stored values to the new type of {}, confirm with convert_field_values.",
                        retyped
                            .iter()
                            .map(|(name, old, new)| format!(
                                "'{name}' ({} → {})",
                                crate::convert::kind_name(old.kind),
                                crate::convert::kind_name(new.kind)
                            ))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                } else {
                    String::new()
                };
                return Err(AppError::conflict_docs(
                    format!(
                        "Existing records do not fit the new fields: {}{hint}",
                        first_error.unwrap_or_default()
                    ),
                    failing,
                ));
            }
            store::update_collection(
                conn,
                &id,
                &name,
                visibility,
                &blueprint,
                &timeutil::now_rfc3339(),
            )
            .await
            .map_err(|err| unique_message(err, "a collection with that name already exists"))?;
            // The previous revision of each record keeps the old values.
            let now = timeutil::now_rfc3339();
            for (doc, source, action) in &stripped {
                store::update_document_as(conn, &id, doc, source, &now, action).await?;
            }
            if !renames.is_empty() {
                crate::records::rename_view_fields(conn, &id, &renames).await?;
            }
            if !value_renames.is_empty() {
                crate::records::rename_view_values(conn, &id, &value_renames).await?;
            }
            // References and formulas elsewhere follow the new names.
            if name != current.name || !renames.is_empty() {
                for mut related in crate::relationships::all_collections(conn).await? {
                    if related.id == id {
                        continue;
                    }
                    let original = related.blueprint.clone();
                    retarget_self(&mut related.blueprint, &current.name, &name);
                    crate::lookups::rename(&mut related.blueprint, &current.name, &name, &renames);
                    if original != related.blueprint {
                        store::update_collection(
                            conn,
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
            store::find_collection(conn, &id)
                .await?
                .ok_or(AppError::NotFound)
        })
        .await?;
    Ok(Json(collection))
}

pub async fn delete_collection(
    State(state): State<AppState>,
    SchemaAdmin(principal): SchemaAdmin,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let conn = state.db.connect().await?;
    let keys=conn.with_app_tx(async |conn| {
        let current=workspace_collection(conn,&principal,&id).await?;
        for related in crate::relationships::all_collections(conn).await?{
            if related.id!=id && reference_collections(&related.blueprint).contains(&current.name.as_str()){
                return Err(AppError::conflict(format!("Collection '{}' has fields referencing this collection. Remove those fields first.",related.name)));
            }
        }
        crate::lookups::check_delete(conn,&id,&current.name).await?;
        let keys=store::asset_keys(conn,&id).await?;
        store::delete_collection(conn,&id).await?;
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
    let collection = workspace_collection(&conn, &principal, &id).await?;
    let mut items = store::list_documents(&conn, &id, limit, offset).await?;
    let lookups = crate::lookups::load(&conn, &collection.blueprint).await?;
    for document in &mut items {
        materialize(&collection.blueprint, &lookups, document);
    }
    Ok(Json(Page {
        items,
        limit,
        offset,
    }))
}

pub async fn create_document(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path(id): Path<String>,
    Json(body): Json<DocumentWrite>,
) -> Result<(StatusCode, Json<DocumentView>), AppError> {
    let conn = state.db.connect().await?;
    let document = conn
        .with_app_tx(async |conn| {
            let collection = workspace_collection(conn, &principal, &id).await?;
            let source = vulse_model::source_document(&collection.blueprint, &body.body);
            check_body(conn, &id, &collection.name, &collection.blueprint, &source).await?;
            let document_id = crate::auth::new_id();
            store::insert_document(conn, &document_id, &id, &source, &timeutil::now_rfc3339())
                .await?;
            let mut document = store::find_document(conn, &id, &document_id)
                .await?
                .ok_or(AppError::NotFound)?;
            let lookups = crate::lookups::load(conn, &collection.blueprint).await?;
            materialize(&collection.blueprint, &lookups, &mut document);
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
    let collection = workspace_collection(&conn, &principal, &id).await?;
    Ok(Json(read_document(&conn, &collection, &doc_id).await?))
}

/// One record with its calculated and lookup values filled in.
pub(crate) async fn read_document(
    conn: &Conn,
    collection: &CollectionView,
    doc_id: &str,
) -> Result<DocumentView, AppError> {
    let mut document = store::find_document(conn, &collection.id, doc_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let lookups = crate::lookups::load(conn, &collection.blueprint).await?;
    materialize(&collection.blueprint, &lookups, &mut document);
    Ok(document)
}

pub async fn update_document(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path((id, doc_id)): Path<(String, String)>,
    Json(body): Json<DocumentWrite>,
) -> Result<Json<DocumentView>, AppError> {
    let conn = state.db.connect().await?;
    let document = conn
        .with_app_tx(async |conn| {
            let collection = workspace_collection(conn, &principal, &id).await?;
            let current = store::find_document(conn, &id, &doc_id)
                .await?
                .ok_or(AppError::NotFound)?;
            crate::mutations::check_version(&current, body.expected_updated_at.as_deref())?;
            let source = vulse_model::source_document(&collection.blueprint, &body.body);
            check_body(conn, &id, &collection.name, &collection.blueprint, &source).await?;
            store::update_document(conn, &id, &doc_id, &source, &timeutil::now_rfc3339()).await?;
            let mut document = store::find_document(conn, &id, &doc_id)
                .await?
                .ok_or(AppError::NotFound)?;
            let lookups = crate::lookups::load(conn, &collection.blueprint).await?;
            materialize(&collection.blueprint, &lookups, &mut document);
            Ok(document)
        })
        .await?;
    Ok(Json(document))
}

pub async fn delete_document(
    State(state): State<AppState>,
    Editor(principal): Editor,
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

/// Looks up a collection and applies an API key's collection scope.
pub async fn workspace_collection(
    conn: &Conn,
    principal: &Principal,
    id: &str,
) -> Result<CollectionView, AppError> {
    let collection = store::find_collection(conn, id)
        .await?
        .ok_or(AppError::NotFound)?;
    require_collection(principal, &collection.id)?;
    Ok(collection)
}

pub(crate) async fn check_body(
    conn: &Conn,
    collection_id: &str,
    collection_name: &str,
    blueprint: &Blueprint,
    body: &serde_json::Value,
) -> Result<(), AppError> {
    let mut found = Found::default();
    check_body_with(
        conn,
        collection_id,
        collection_name,
        blueprint,
        body,
        &mut found,
    )
    .await
}

/// Collections (name → id) and records that references were found to point
/// at, so checking many bodies in one transaction looks each up once.
#[derive(Default)]
pub(crate) struct Found {
    collections: std::collections::HashMap<String, String>,
    documents: std::collections::HashSet<(String, String)>,
}

/// Like `check_body`, remembering what was found in `found`.
pub(crate) async fn check_body_with(
    conn: &Conn,
    collection_id: &str,
    collection_name: &str,
    blueprint: &Blueprint,
    body: &serde_json::Value,
    found: &mut Found,
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
        } else if let Some(id) = found.collections.get(&reference.collection) {
            id.clone()
        } else {
            let id = store::find_collection_by_name(conn, &reference.collection)
                .await?
                .ok_or_else(|| {
                    AppError::bad(format!("unknown collection '{}'", reference.collection))
                })?
                .id;
            found
                .collections
                .insert(reference.collection.clone(), id.clone());
            id
        };
        let key = (target_id, reference.document_id);
        if found.documents.contains(&key) {
            continue;
        }
        if !store::document_exists(conn, &key.0, &key.1).await? {
            return Err(AppError::bad(format!("unknown document {}", key.1)));
        }
        found.documents.insert(key);
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
    blueprint: &Blueprint,
    self_name: &str,
) -> Result<(), AppError> {
    for name in reference_collections(blueprint) {
        if name == self_name {
            continue;
        }
        if store::find_collection_by_name(conn, name).await?.is_none() {
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

/// Fills in calculated fields; `lookups` holds the columns their formulas
/// read from collections (see `crate::lookups::load`).
pub(crate) fn materialize(
    blueprint: &Blueprint,
    lookups: &vulse_model::Lookups,
    document: &mut DocumentView,
) {
    let (body, errors) = vulse_model::calculate_document_with(blueprint, &document.body, lookups);
    document.body = body;
    document.calculation_errors = errors;
}
