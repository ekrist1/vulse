use crate::{
    auth::{AuthUser, Editor},
    content::{PageQuery, check_body, materialize, page_query, workspace_collection},
    db::{Conn, opt_text_at, text_at},
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
    let source = conn.source().map_or(crate::db::SqlValue::Null, text);
    let Some(stored) = store::stored_collection(conn, collection).await? else {
        return Ok(());
    };
    let blueprint = revision_blueprint(conn, &stored).await?;
    conn.execute("INSERT INTO record_revisions(id,collection_id,document_id,action,body,blueprint,record_created_at,created_at,source) SELECT ?1,d.collection_id,d.id,?2,d.body,?7,d.created_at,?3,?6 FROM documents d WHERE d.collection_id=?4 AND d.id=?5",&[text(&crate::auth::new_id()),text(action),text(&timeutil::now_rfc3339()),text(collection),text(document),source,text(&blueprint)]).await?;
    Ok(())
}

/// Stores a collection's blueprint text once for the revisions made with
/// it, and returns the id that `record_revisions.blueprint` holds.
pub(crate) async fn store_blueprint(conn: &Conn, blueprint: &str) -> anyhow::Result<String> {
    use sha2::Digest;
    let id = hex::encode(sha2::Sha256::digest(blueprint.as_bytes()));
    conn.execute(
        "INSERT OR IGNORE INTO revision_blueprints(id,blueprint) VALUES(?1,?2)",
        &[text(&id), text(blueprint)],
    )
    .await?;
    Ok(id)
}
/// Inserts the revisions `snapshot` would copy from records, given as
/// (record id, action, stored body, record created_at), in batches.
pub(crate) async fn insert_revisions(
    conn: &Conn,
    collection: &str,
    blueprint: &str,
    revisions: &[(&str, &str, &str, &str)],
) -> anyhow::Result<()> {
    let source = conn.source().map_or(crate::db::SqlValue::Null, text);
    let revised_at = timeutil::now_rfc3339();
    // Parameters per statement, as for record table rows.
    for chunk in revisions.chunks(2000) {
        let mut params = Vec::with_capacity(chunk.len() * 9);
        let mut tuples = Vec::with_capacity(chunk.len());
        for (document, action, body, record_created_at) in chunk {
            let n = params.len();
            tuples.push(format!(
                "({})",
                (n + 1..=n + 9)
                    .map(|i| format!("?{i}"))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
            params.extend([
                text(&crate::auth::new_id()),
                text(collection),
                text(document),
                text(action),
                text(body),
                text(blueprint),
                text(record_created_at),
                text(&revised_at),
                source.clone(),
            ]);
        }
        conn.execute(
            &format!(
                "INSERT INTO record_revisions(id,collection_id,document_id,action,body,blueprint,record_created_at,created_at,source) VALUES {}",
                tuples.join(",")
            ),
            &params,
        )
        .await?;
    }
    Ok(())
}

/// `store_blueprint` for a collection's current blueprint, stored once per
/// transaction.
pub(crate) async fn revision_blueprint(
    conn: &Conn,
    collection: &store::StoredCollection,
) -> anyhow::Result<String> {
    if let Some(id) = collection.revision_blueprint.get() {
        return Ok(id.clone());
    }
    let id = store_blueprint(conn, &collection.blueprint_text).await?;
    let _ = collection.revision_blueprint.set(id.clone());
    Ok(id)
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
        source: opt_text_at(row, 7)?,
    })
}
// Revisions hold the id of a stored blueprint; rows that predate it hold the text.
const COLUMNS: &str = "r.id,r.document_id,r.action,r.body,COALESCE((SELECT b.blueprint FROM revision_blueprints b WHERE b.id=r.blueprint),r.blueprint),r.record_created_at,r.created_at,r.source";
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
 let source = vulse_model::source_document(&collection.blueprint, &revision.body);
 vulse_model::validate_document(&collection.blueprint, &source).map_err(AppError::bad)?;
 let now = timeutil::now_rfc3339();
 // Insert first to permit a deleted record's self reference; the transaction rolls back on validation errors.
 if let Some(current) = &current {
     check_body(conn, &id, &collection.name, &collection.blueprint, &source).await?;
     let same = current.body == source;
     store::update_document_as(conn, &id, &doc, &source, &now, "restore").await?;
     if same {
         snapshot(conn, &id, &doc, "restore").await?;
     }
 } else {
     store::insert_document_as(
         conn,
         &doc,
         &id,
         &source,
         &revision.record_created_at,
         &now,
         "restore",
     )
     .await?;
     check_body(conn, &id, &collection.name, &collection.blueprint, &source).await?;
 }
 let mut result=store::find_document(conn,&id,&doc).await?.ok_or(AppError::NotFound)?;let lookups=crate::lookups::load(conn,&collection.blueprint).await?;materialize(&collection.blueprint,&lookups,&mut result);Ok(result)
 }).await?;
    Ok(Json(result))
}

fn text(v: &str) -> crate::db::SqlValue {
    crate::db::SqlValue::Text(v.into())
}
