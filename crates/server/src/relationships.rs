use crate::{
    auth::AuthUser,
    content::{PageQuery, page_query, workspace_collection},
    db::{Conn, SqlValue, text_at},
    error::AppError,
    state::AppState,
    store,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use vulse_model::{Blueprint, CollectionView, Page, ReferenceView};
fn text(v: &str) -> SqlValue {
    SqlValue::Text(v.into())
}
pub async fn all_collections(conn: &Conn) -> anyhow::Result<Vec<CollectionView>> {
    let mut all = vec![];
    loop {
        let page = store::list_collections(conn, 100, all.len() as u32).await?;
        let done = page.len() < 100;
        all.extend(page);
        if done {
            return Ok(all);
        }
    }
}
pub async fn sync(
    conn: &Conn,
    collection: &str,
    document: &str,
    blueprint: &Blueprint,
    body: &serde_json::Value,
) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM document_links WHERE source_id=?1",
        &[text(document)],
    )
    .await?;
    let mut targets = Targets::new(conn, collection).await?;
    for (field, kind, target_collection, target) in
        record_links(conn, collection, blueprint, body, &mut targets).await?
    {
        insert_link(conn, document, &field, kind, &target_collection, &target).await?;
    }
    Ok(())
}

/// Writes the links of records that have none yet, such as new records or
/// a collection whose links were all removed, with multi-row inserts.
pub(crate) async fn insert_many(
    conn: &Conn,
    collection: &str,
    blueprint: &Blueprint,
    records: &[(String, serde_json::Value)],
) -> anyhow::Result<()> {
    if !has_links(blueprint) {
        return Ok(());
    }
    let mut targets = Targets::new(conn, collection).await?;
    let mut params = Vec::new();
    let mut tuples = Vec::new();
    for (document, body) in records {
        for (field, kind, target_collection, target) in
            record_links(conn, collection, blueprint, body, &mut targets).await?
        {
            let n = params.len();
            tuples.push(format!(
                "(?{},?{},?{},?{},?{})",
                n + 1,
                n + 2,
                n + 3,
                n + 4,
                n + 5
            ));
            params.extend([
                text(document),
                text(&field),
                text(kind),
                text(&target_collection),
                text(&target),
            ]);
            if tuples.len() == 2000 {
                insert_links(conn, &mut tuples, &mut params).await?;
            }
        }
    }
    insert_links(conn, &mut tuples, &mut params).await
}

async fn insert_links(
    conn: &Conn,
    tuples: &mut Vec<String>,
    params: &mut Vec<SqlValue>,
) -> anyhow::Result<()> {
    if tuples.is_empty() {
        return Ok(());
    }
    conn.execute(
        &format!(
            "INSERT OR IGNORE INTO document_links(source_id,field,kind,target_collection,target_id) VALUES {}",
            tuples.join(",")
        ),
        params,
    )
    .await?;
    tuples.clear();
    params.clear();
    Ok(())
}

/// The ids of collections that records link to, by name, found once.
struct Targets {
    name: String,
    ids: std::collections::HashMap<String, Option<String>>,
}

impl Targets {
    async fn new(conn: &Conn, collection: &str) -> anyhow::Result<Self> {
        let rows = conn
            .query(
                "SELECT name FROM collections WHERE id=?1",
                &[text(collection)],
            )
            .await?;
        let row = rows
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing collection"))?;
        Ok(Self {
            name: text_at(row, 0)?,
            ids: Default::default(),
        })
    }
}

/// A record's links as (field, kind, target collection id, target id).
async fn record_links(
    conn: &Conn,
    collection: &str,
    blueprint: &Blueprint,
    body: &serde_json::Value,
    targets: &mut Targets,
) -> anyhow::Result<Vec<(String, &'static str, String, String)>> {
    let mut out = Vec::new();
    for field in &blueprint.fields {
        if !links(field) {
            continue;
        }
        let mut single = field.clone();
        single.required = false;
        let checked = vulse_model::validate_document(
            &Blueprint {
                fields: vec![single],
                title_field: None,
            },
            &serde_json::json!({field.name.clone():body[&field.name]}),
        )
        .map_err(anyhow::Error::msg)?;
        for asset in checked.asset_ids {
            out.push((field.name.clone(), "asset", collection.to_string(), asset));
        }
        for reference in checked.references {
            let target = if reference.collection == targets.name {
                Some(collection.to_string())
            } else if let Some(id) = targets.ids.get(&reference.collection) {
                id.clone()
            } else {
                let id = store::find_collection_by_name(conn, &reference.collection)
                    .await?
                    .map(|c| c.id);
                targets.ids.insert(reference.collection.clone(), id.clone());
                id
            };
            if let Some(target) = target {
                out.push((
                    field.name.clone(),
                    "document",
                    target,
                    reference.document_id,
                ));
            }
        }
    }
    Ok(out)
}
/// Whether a field can link to records or assets.
fn links(field: &vulse_model::FieldDef) -> bool {
    matches!(
        field.kind,
        vulse_model::FieldType::Reference
            | vulse_model::FieldType::Asset
            | vulse_model::FieldType::PortableText
    ) || (field.kind == vulse_model::FieldType::Array
        && matches!(
            field.items,
            Some(vulse_model::ArrayItems::Reference | vulse_model::ArrayItems::Asset)
        ))
}
pub(crate) fn has_links(blueprint: &Blueprint) -> bool {
    blueprint.fields.iter().any(links)
}
async fn insert_link(
    conn: &Conn,
    source: &str,
    field: &str,
    kind: &str,
    target_collection: &str,
    target: &str,
) -> anyhow::Result<()> {
    conn.execute("INSERT OR IGNORE INTO document_links(source_id,field,kind,target_collection,target_id) VALUES(?1,?2,?3,?4,?5)",&[text(source),text(field),text(kind),text(target_collection),text(target)]).await?;
    Ok(())
}
pub async fn rebuild(conn: &Conn) -> anyhow::Result<()> {
    for row in conn
        .query("SELECT id,blueprint FROM collections", &[])
        .await?
    {
        let id = text_at(&row, 0)?;
        let blueprint = serde_json::from_str(&text_at(&row, 1)?)?;
        for (doc, body) in store::list_document_bodies(conn, &id).await? {
            sync(conn, &id, &doc, &blueprint, &body).await?;
        }
    }
    Ok(())
}
pub async fn guard_delete(
    conn: &Conn,
    collection: &str,
    ids: &[String],
    kind: &str,
) -> Result<(), AppError> {
    let mut sources = std::collections::BTreeSet::new();
    for id in ids {
        for row in conn.query("SELECT source_id FROM document_links WHERE kind=?1 AND target_collection=?2 AND target_id=?3",&[text(kind),text(collection),text(id)]).await?{let source=text_at(&row,0)?;if kind!="document"||!ids.contains(&source){sources.insert(source);}}
    }
    if !sources.is_empty() {
        return Err(AppError::conflict_docs(
            "Remove references from other records before deleting this content",
            sources.into_iter().collect(),
        ));
    }
    Ok(())
}
pub async fn backlinks(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path((id, doc)): Path<(String, String)>,
    Query(query): Query<PageQuery>,
) -> Result<Json<Page<ReferenceView>>, AppError> {
    let conn = state.db.connect().await?;
    workspace_collection(&conn, &principal, &id).await?;
    if !store::document_exists(&conn, &id, &doc).await? {
        return Err(AppError::NotFound);
    }
    let (limit, offset) = page_query(&query)?;
    let rows=conn.query("SELECT c.id,c.name,d.id,d.body,c.blueprint,l.field FROM document_links l JOIN documents d ON d.id=l.source_id JOIN collections c ON c.id=d.collection_id WHERE l.kind='document' AND l.target_collection=?1 AND l.target_id=?2 AND (?3 IS NULL OR c.id=?3) ORDER BY c.name,d.id,l.field LIMIT ?4 OFFSET ?5",&[text(&id),text(&doc),principal.collection_scope.as_deref().map(text).unwrap_or(SqlValue::Null),SqlValue::Integer(limit as i64),SqlValue::Integer(offset as i64)]).await?;
    let items = rows
        .iter()
        .map(|row| -> anyhow::Result<ReferenceView> {
            let blueprint: Blueprint = serde_json::from_str(&text_at(row, 4)?)?;
            let body = serde_json::from_str(&text_at(row, 3)?)?;
            let document_id = text_at(row, 2)?;
            let field = text_at(row, 5)?;
            Ok(ReferenceView {
                collection_id: text_at(row, 0)?,
                collection_name: text_at(row, 1)?,
                title: vulse_model::record_title(
                    &blueprint,
                    &vulse_model::calculate_document(&blueprint, &body).0,
                    &document_id,
                ),
                document_id,
                field: blueprint
                    .fields
                    .iter()
                    .find(|f| f.name == field)
                    .map(|f| f.display_label().to_string())
                    .unwrap_or(field),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Json(Page {
        items,
        limit,
        offset,
    }))
}
