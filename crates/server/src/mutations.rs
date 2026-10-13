use crate::{
    auth::{Editor, Principal},
    content::{check_body, materialize, workspace_collection},
    db::{Conn, SqlValue, text_at},
    error::AppError,
    state::AppState,
    store, timeutil,
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vulse_model::{BulkWrite, DocumentPatch, DocumentView, FieldType, MutationResult};
#[derive(Serialize, Deserialize)]
struct Snapshot {
    before: Vec<DocumentView>,
    after: Vec<DocumentView>,
}
fn text(value: &str) -> SqlValue {
    SqlValue::Text(value.into())
}
pub(crate) fn check_version(
    document: &DocumentView,
    expected: Option<&str>,
) -> Result<(), AppError> {
    if expected.is_some_and(|expected| expected != document.updated_at) {
        return Err(AppError::conflict(
            "This record changed since you loaded it. Reload before saving.",
        ));
    }
    Ok(())
}
fn merged(
    blueprint: &vulse_model::Blueprint,
    body: &Value,
    patch: &Value,
) -> Result<Value, AppError> {
    let patch = patch
        .as_object()
        .ok_or_else(|| AppError::bad("patch must be an object"))?;
    let mut body = body
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::bad("document must be an object"))?;
    for (key, value) in patch {
        let field = blueprint
            .fields
            .iter()
            .find(|f| f.name == *key)
            .ok_or_else(|| AppError::bad(format!("unknown field '{key}'")))?;
        if field.kind == FieldType::Calculated {
            return Err(AppError::bad("calculated fields are read-only"));
        }
        body.insert(key.clone(), value.clone());
    }
    Ok(Value::Object(body))
}
async fn snapshot(
    conn: &Conn,
    collection: &str,
    before: Vec<DocumentView>,
    after: Vec<DocumentView>,
) -> Result<String, AppError> {
    let id = crate::auth::new_id();
    let value = serde_json::to_string(&Snapshot { before, after }).map_err(anyhow::Error::from)?;
    conn.execute(
        "INSERT INTO record_actions(id,collection_id,snapshot,created_at) VALUES(?1,?2,?3,?4)",
        &[
            text(&id),
            text(collection),
            text(&value),
            text(&timeutil::now_rfc3339()),
        ],
    )
    .await?;
    Ok(id)
}
pub async fn patch_document(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path((id, doc_id)): Path<(String, String)>,
    Json(body): Json<DocumentPatch>,
) -> Result<Json<MutationResult>, AppError> {
    let expected = body
        .expected_updated_at
        .map(|version| [(doc_id.clone(), version)].into_iter().collect())
        .unwrap_or_default();
    Ok(Json(
        run_bulk(
            &state,
            &principal,
            &id,
            BulkWrite {
                action: "update".into(),
                ids: vec![doc_id],
                patch: body.patch,
                expected,
            },
        )
        .await?,
    ))
}
pub async fn bulk_records(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path(id): Path<String>,
    Json(body): Json<BulkWrite>,
) -> Result<Json<MutationResult>, AppError> {
    Ok(Json(run_bulk(&state, &principal, &id, body).await?))
}
pub(crate) async fn run_bulk(
    state: &AppState,
    principal: &Principal,
    id: &str,
    body: BulkWrite,
) -> Result<MutationResult, AppError> {
    if body.ids.is_empty()
        || body.ids.len() > 100
        || body
            .ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != body.ids.len()
    {
        return Err(AppError::bad("choose 1–100 unique records"));
    }
    if !["update", "duplicate", "delete"].contains(&body.action.as_str()) {
        return Err(AppError::bad("unknown bulk action"));
    }
    let conn = state.db.connect().await?;
    conn.with_app_tx(async |conn| {
        let collection = workspace_collection(conn, principal, id).await?;
        let mut before = vec![];
        for doc_id in &body.ids {
            let doc = store::find_document(conn, id, doc_id)
                .await?
                .ok_or(AppError::NotFound)?;
            check_version(&doc, body.expected.get(doc_id).map(String::as_str))?;
            before.push(doc);
        }
        let mut after = vec![];
        let now = timeutil::now_rfc3339();
        // Validate every update before writing any of the selected records.
        let mut writes = vec![];
        if body.action != "delete" {
            for doc in &before {
                let next = if body.action == "update" {
                    merged(&collection.blueprint, &doc.body, &body.patch)?
                } else {
                    doc.body.clone()
                };
                check_body(conn, id, &collection.name, &collection.blueprint, &next).await?;
                writes.push((
                    if body.action == "duplicate" {
                        crate::auth::new_id()
                    } else {
                        doc.id.clone()
                    },
                    next,
                ));
            }
        }
        if body.action == "delete" {
            crate::relationships::guard_delete(conn, id, &body.ids, "document").await?;
            for doc in &before {
                store::delete_document(conn, id, &doc.id).await?;
            }
        } else {
            for (doc_id, next) in writes {
                if body.action == "duplicate" {
                    store::insert_document(conn, &doc_id, id, &next, &now).await?;
                } else {
                    store::update_document(conn, id, &doc_id, &next, &now).await?;
                }
                after.push(
                    store::find_document(conn, id, &doc_id)
                        .await?
                        .ok_or(AppError::NotFound)?,
                );
            }
        }
        let undo_id = snapshot(
            conn,
            id,
            if body.action == "duplicate" {
                vec![]
            } else {
                before
            },
            after.clone(),
        )
        .await?;
        let lookups = crate::lookups::load(conn, &collection.blueprint).await?;
        for doc in &mut after {
            materialize(&collection.blueprint, &lookups, doc);
        }
        Ok(MutationResult {
            documents: after,
            affected: body.ids.len(),
            undo_id,
        })
    })
    .await
}
pub async fn undo_action(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path((id, action_id)): Path<(String, String)>,
) -> Result<Json<MutationResult>, AppError> {
    let conn = state.db.connect().await?;
    let result=conn.with_app_tx(async |conn|{
        let collection=workspace_collection(conn,&principal,&id).await?;
        let rows=conn.query("SELECT snapshot,created_at FROM record_actions WHERE id=?1 AND collection_id=?2",&[text(&action_id),text(&id)]).await?;let row=rows.first().ok_or(AppError::NotFound)?;
        let time=chrono::DateTime::parse_from_rfc3339(&text_at(row,1)?).map_err(anyhow::Error::from)?;
        if chrono::Utc::now().signed_duration_since(time).num_hours()>=24{return Err(AppError::conflict("This undo action has expired."));}
        let snapshot:Snapshot=serde_json::from_str(&text_at(row,0)?).map_err(anyhow::Error::from)?;
        for doc in &snapshot.after {let current=store::find_document(conn,&id,&doc.id).await?.ok_or_else(||AppError::conflict("An affected record has since been deleted."))?;if current.updated_at!=doc.updated_at||current.body!=doc.body{return Err(AppError::conflict("An affected record changed after this action. Use revision history to recover earlier content."));}}
        for doc in &snapshot.before {if !snapshot.after.iter().any(|after|after.id==doc.id)&&store::document_exists(conn,&id,&doc.id).await?{return Err(AppError::conflict("A deleted record has already been restored."));}}
        let deletions:Vec<_>=snapshot.after.iter().filter(|d|!snapshot.before.iter().any(|b|b.id==d.id)).map(|d|d.id.clone()).collect();
        crate::relationships::guard_delete(conn,&id,&deletions,"document").await?;
        let now=timeutil::now_rfc3339();
        for doc in &snapshot.after {if !snapshot.before.iter().any(|before|before.id==doc.id){store::delete_document(conn,&id,&doc.id).await?;}}
        for doc in &snapshot.before {if !store::document_exists(conn,&id,&doc.id).await?{store::insert_document(conn,&doc.id,&id,&doc.body,&doc.created_at).await?;}store::update_document(conn,&id,&doc.id,&doc.body,&now).await?;}
        for doc in &snapshot.before {check_body(conn,&id,&collection.name,&collection.blueprint,&doc.body).await?;}
        conn.execute("DELETE FROM record_actions WHERE id=?1",&[text(&action_id)]).await?;
        let lookups=crate::lookups::load(conn,&collection.blueprint).await?;let mut documents=vec![];for doc in &snapshot.before {let mut current=store::find_document(conn,&id,&doc.id).await?.ok_or(AppError::NotFound)?;materialize(&collection.blueprint,&lookups,&mut current);documents.push(current);}
        Ok(MutationResult{affected:snapshot.before.len().max(snapshot.after.len()),documents,undo_id:String::new()})
    }).await?;
    Ok(Json(result))
}
