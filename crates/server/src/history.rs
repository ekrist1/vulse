use crate::{
    auth::{AuthUser, Editor},
    content::{PageQuery, check_body, materialize, page_query, workspace_collection},
    db::{Conn, text_at},
    error::AppError,
    state::AppState,
    store, timeutil,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use vulse_model::{DocumentRevision, DocumentView, Page, RestoreRevision};

pub async fn snapshot(
    conn: &Conn,
    collection: &str,
    document: &str,
    action: &str,
) -> anyhow::Result<()> {
    conn.execute("INSERT INTO record_revisions(id,collection_id,document_id,action,body,blueprint,record_created_at,created_at) SELECT ?1,d.collection_id,d.id,?2,d.body,c.blueprint,d.created_at,?3 FROM documents d JOIN collections c ON c.id=d.collection_id WHERE d.collection_id=?4 AND d.id=?5",&[text(&crate::auth::new_id()),text(action),text(&timeutil::now_rfc3339()),text(collection),text(document)]).await?;
    Ok(())
}
fn decode(row: &crate::db::SqlRow) -> anyhow::Result<DocumentRevision> {
    Ok(DocumentRevision {
        id: text_at(row, 0)?,
        document_id: text_at(row, 1)?,
        action: text_at(row, 2)?,
        body: serde_json::from_str(&text_at(row, 3)?)?,
        blueprint: serde_json::from_str(&text_at(row, 4)?)?,
        record_created_at: text_at(row, 5)?,
        created_at: text_at(row, 6)?,
    })
}
const COLUMNS: &str =
    "r.id,r.document_id,r.action,r.body,r.blueprint,r.record_created_at,r.created_at";
pub async fn list(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path((id, doc)): Path<(String, String)>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<DocumentRevision>>, AppError> {
    let conn = state.db.connect().await?;
    workspace_collection(&conn, &principal, &id).await?;
    let (limit, offset) = page_query(&query)?;
    let rows=conn.query(&format!("SELECT {COLUMNS} FROM record_revisions r WHERE r.collection_id=?1 AND r.document_id=?2 ORDER BY r.created_at DESC,r.id DESC LIMIT ?3 OFFSET ?4"),&[text(&id),text(&doc),crate::db::SqlValue::Integer(limit as i64),crate::db::SqlValue::Integer(offset as i64)]).await?;
    Ok(Json(Page {
        items: rows.iter().map(decode).collect::<anyhow::Result<_>>()?,
        limit,
        offset,
    }))
}
pub async fn deleted(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<DocumentRevision>>, AppError> {
    let conn = state.db.connect().await?;
    workspace_collection(&conn, &principal, &id).await?;
    let (limit, offset) = page_query(&query)?;
    let rows=conn.query(&format!("SELECT {COLUMNS} FROM record_revisions r WHERE r.collection_id=?1 AND r.action='delete' AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.id=r.document_id AND d.collection_id=r.collection_id) AND r.id=(SELECT h.id FROM record_revisions h WHERE h.collection_id=r.collection_id AND h.document_id=r.document_id ORDER BY h.created_at DESC,h.id DESC LIMIT 1) ORDER BY r.created_at DESC,r.id DESC LIMIT ?2 OFFSET ?3"),&[text(&id),crate::db::SqlValue::Integer(limit as i64),crate::db::SqlValue::Integer(offset as i64)]).await?;
    Ok(Json(Page {
        items: rows.iter().map(decode).collect::<anyhow::Result<_>>()?,
        limit,
        offset,
    }))
}
pub async fn restore(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path((id, doc, revision)): Path<(String, String, String)>,
    Json(write): Json<RestoreRevision>,
) -> Result<Json<DocumentView>, AppError> {
    let conn = state.db.connect().await?;
    let result=conn.with_app_tx(async |conn|{
 let collection=workspace_collection(conn,&principal,&id).await?;
 let rows=conn.query(&format!("SELECT {COLUMNS} FROM record_revisions r WHERE r.id=?1 AND r.collection_id=?2 AND r.document_id=?3"),&[text(&revision),text(&id),text(&doc)]).await?;
 let revision=rows.first().map(decode).transpose()?.ok_or(AppError::NotFound)?;
 let current=store::find_document(conn,&id,&doc).await?;
 if let Some(current)=&current {if write.expected_updated_at.is_none(){return Err(AppError::conflict("Reload the record before restoring a version"));}crate::mutations::check_version(current,write.expected_updated_at.as_deref())?;}
 else if write.expected_updated_at.is_some(){return Err(AppError::conflict("The record was deleted. Restore it from Trash."));}
 let source=vulse_model::source_document(&collection.blueprint,&revision.body);
 // Insert first to permit a deleted record's self reference; the transaction rolls back on validation errors.
 if current.is_none(){store::insert_document(conn,&doc,&id,&source,&revision.record_created_at).await?;}
 check_body(conn,&id,&collection.name,&collection.blueprint,&source).await?;
 store::update_document(conn,&id,&doc,&source,&timeutil::now_rfc3339()).await?;
 snapshot(conn,&id,&doc,"restore").await?;
 let mut result=store::find_document(conn,&id,&doc).await?.ok_or(AppError::NotFound)?;materialize(&collection.blueprint,&mut result);Ok(result)
 }).await?;
    Ok(Json(result))
}

fn text(v: &str) -> crate::db::SqlValue {
    crate::db::SqlValue::Text(v.into())
}
