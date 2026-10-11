use crate::{
    auth::{AuthUser, Editor},
    content::{materialize, workspace_collection},
    db::{Conn, SqlValue, int_at, text_at},
    error::{AppError, unique_message},
    state::AppState,
    store,
};
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, State},
};
use vulse_model::{
    Blueprint, FieldType, RecordPage, RecordQuery, SavedView, SavedViewWrite, validate_query,
    validate_view,
};
fn text(value: &str) -> SqlValue {
    SqlValue::Text(value.into())
}

pub async fn sync_cells(
    conn: &Conn,
    blueprint: &Blueprint,
    id: &str,
    body: &serde_json::Value,
) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM document_cells WHERE document_id = ?1",
        &[text(id)],
    )
    .await?;
    let (body, _) = vulse_model::calculate_document(blueprint, body);
    for field in &blueprint.fields {
        let value = &body[&field.name];
        if value.is_null() {
            continue;
        }
        conn.execute("INSERT INTO document_cells(document_id,field,text_value,raw_value,number_value) VALUES(?1,?2,?3,?4,?5)", &[text(id),text(&field.name),text(&vulse_model::display_value(value).to_lowercase()),text(&value.to_string()),if value.is_number(){text(&value.to_string())}else{SqlValue::Null}]).await?;
    }
    Ok(())
}
pub async fn sync_collection_cells(
    conn: &Conn,
    id: &str,
    blueprint: &Blueprint,
) -> anyhow::Result<()> {
    for (document, body) in store::list_document_bodies(conn, id).await? {
        sync_cells(conn, blueprint, &document, &body).await?;
        crate::relationships::sync(conn, id, &document, blueprint, &body).await?;
    }
    Ok(())
}
pub async fn rebuild_all_cells(conn: &Conn) -> anyhow::Result<()> {
    for row in conn
        .query("SELECT id,blueprint FROM collections", &[])
        .await?
    {
        let blueprint = serde_json::from_str(&text_at(&row, 1)?)?;
        for (doc, body) in store::list_document_bodies(conn, &text_at(&row, 0)?).await? {
            sync_cells(conn, &blueprint, &doc, &body).await?;
        }
    }
    Ok(())
}
pub async fn sync_stored_document(
    conn: &Conn,
    collection: &str,
    id: &str,
    body: &serde_json::Value,
) -> anyhow::Result<()> {
    let rows = conn
        .query(
            "SELECT blueprint FROM collections WHERE id=?1",
            &[text(collection)],
        )
        .await?;
    let row = rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing collection"))?;
    let blueprint = serde_json::from_str(&text_at(row, 0)?)?;
    sync_cells(conn, &blueprint, id, body).await?;
    crate::relationships::sync(conn, collection, id, &blueprint, body).await
}

fn parameter(params: &mut Vec<SqlValue>, value: SqlValue) -> String {
    params.push(value);
    format!("?{}", params.len())
}
fn like(value: &str) -> String {
    format!(
        "%{}%",
        value
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}
pub(crate) fn query_sql(
    blueprint: &Blueprint,
    id: &str,
    query: &RecordQuery,
) -> Result<(String, String, Vec<SqlValue>), AppError> {
    validate_query(blueprint, query).map_err(AppError::bad)?;
    let mut params = vec![text(id)];
    let mut conditions = vec!["d.collection_id=?1".to_string()];
    if !query.search.trim().is_empty() {
        let p = parameter(&mut params, text(&like(query.search.trim())));
        conditions.push(format!("(lower(d.id) LIKE {p} ESCAPE '\\' OR EXISTS(SELECT 1 FROM document_cells c WHERE c.document_id=d.id AND c.text_value LIKE {p} ESCAPE '\\'))"));
    }
    for filter in &query.filters {
        let is_date = blueprint
            .fields
            .iter()
            .any(|f| f.name == filter.field && f.kind == FieldType::Date);
        let field = parameter(&mut params, text(&filter.field));
        let predicate = if ["empty", "not_empty"].contains(&filter.op.as_str()) {
            "c.text_value<>'' AND c.raw_value<>'[]' AND c.raw_value<>'{}'".into()
        } else {
            let value = if filter.op == "contains" {
                like(&vulse_model::display_value(&filter.value))
            } else {
                vulse_model::display_value(&filter.value).to_lowercase()
            };
            let p = parameter(&mut params, text(&value));
            match filter.op.as_str() {
                "contains" => format!("c.text_value LIKE {p} ESCAPE '\\'"),
                "eq" | "neq" if filter.value.is_number() => format!(
                    "c.number_value IS NOT NULL AND CAST(c.number_value AS REAL)=CAST({p} AS REAL)"
                ),
                "eq" | "neq" => format!("c.text_value={p}"),
                // ISO dates order correctly as text.
                op if is_date => format!(
                    "c.text_value<>'' AND c.text_value{}{p}",
                    match op {
                        "gt" => ">",
                        "gte" => ">=",
                        "lt" => "<",
                        _ => "<=",
                    }
                ),
                op => format!(
                    "c.number_value IS NOT NULL AND CAST(c.number_value AS REAL){}CAST({p} AS REAL)",
                    match op {
                        "gt" => ">",
                        "gte" => ">=",
                        "lt" => "<",
                        _ => "<=",
                    }
                ),
            }
        };
        conditions.push(format!("{}EXISTS(SELECT 1 FROM document_cells c WHERE c.document_id=d.id AND c.field={field} AND {predicate})",if matches!(filter.op.as_str(),"empty"|"neq"){"NOT "}else{""}));
    }
    let where_sql = conditions.join(" AND ");
    let mut joins = String::new();
    let mut order = vec![];
    if !query.group_field.is_empty() {
        let p = parameter(&mut params, text(&query.group_field));
        joins.push_str(&format!(
            " LEFT JOIN document_cells g ON g.document_id=d.id AND g.field={p}"
        ));
        order.push("g.text_value ASC".into());
    }
    let direction = if query.descending { "DESC" } else { "ASC" };
    if ["created_at", "updated_at", "id"].contains(&query.sort_field.as_str()) {
        order.push(format!("d.{} {direction}", query.sort_field));
    } else {
        let p = parameter(&mut params, text(&query.sort_field));
        joins.push_str(&format!(
            " LEFT JOIN document_cells s ON s.document_id=d.id AND s.field={p}"
        ));
        order.push("(s.document_id IS NULL) ASC".into());
        let numeric = blueprint
            .fields
            .iter()
            .find(|f| f.name == query.sort_field)
            .is_some_and(|f| matches!(f.kind, FieldType::Number | FieldType::Calculated));
        order.push(format!("{} {direction}",if numeric{"CASE WHEN s.number_value IS NOT NULL THEN CAST(s.number_value AS REAL) ELSE s.text_value END"}else{"s.text_value"}));
    }
    order.push("d.id ASC".into());
    let from = format!("FROM documents d{joins} WHERE {where_sql}");
    Ok((from, order.join(","), params))
}
pub(crate) async fn run_query(
    conn: &Conn,
    collection: &vulse_model::CollectionView,
    query: &RecordQuery,
) -> Result<RecordPage, AppError> {
    let (from, order, mut params) = query_sql(&collection.blueprint, &collection.id, query)?;
    let count = conn
        .query(&format!("SELECT count(*) {from}"), &params)
        .await?;
    let total = int_at(&count[0], 0)? as u64;
    let limit = parameter(&mut params, SqlValue::Integer(query.limit as i64));
    let offset = parameter(&mut params, SqlValue::Integer(query.offset as i64));
    let rows=conn.query(&format!("SELECT d.id,d.body,d.created_at,d.updated_at {from} ORDER BY {order} LIMIT {limit} OFFSET {offset}"),&params).await?;
    let mut items = rows
        .iter()
        .map(store::document_from_row)
        .collect::<anyhow::Result<Vec<_>>>()?;
    for doc in &mut items {
        materialize(&collection.blueprint, doc);
    }
    Ok(RecordPage {
        items,
        total,
        limit: query.limit,
        offset: query.offset,
    })
}
pub async fn query_records(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path(id): Path<String>,
    Json(query): Json<RecordQuery>,
) -> Result<Json<RecordPage>, AppError> {
    let conn = state.db.connect().await?;
    let collection = workspace_collection(&conn, &principal, &id).await?;
    Ok(Json(run_query(&conn, &collection, &query).await?))
}
pub async fn list_views(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path(id): Path<String>,
) -> Result<Json<Vec<SavedView>>, AppError> {
    let conn = state.db.connect().await?;
    workspace_collection(&conn, &principal, &id).await?;
    let rows = conn
        .query(
            "SELECT id,config FROM collection_views WHERE collection_id=?1 ORDER BY name,id",
            &[text(&id)],
        )
        .await?;
    Ok(Json(
        rows.iter()
            .map(|row| {
                Ok(SavedView {
                    id: text_at(row, 0)?,
                    config: serde_json::from_str(&text_at(row, 1)?)?,
                })
            })
            .collect::<anyhow::Result<_>>()?,
    ))
}
async fn write_view(
    state: AppState,
    principal: crate::auth::Principal,
    id: String,
    view_id: Option<String>,
    mut config: SavedViewWrite,
) -> Result<SavedView, AppError> {
    let conn = state.db.connect().await?;
    conn.with_app_tx(async |conn| {
        let collection=workspace_collection(conn,&principal,&id).await?;config.name=config.name.trim().into();config.query.offset=0;validate_view(&collection.blueprint,&config).map_err(AppError::bad)?;
        let view=SavedView{id:view_id.clone().unwrap_or_else(crate::auth::new_id),config};
        let encoded=serde_json::to_string(&view.config).map_err(anyhow::Error::from)?;
        if view_id.is_some() {let changed=conn.execute("UPDATE collection_views SET name=?1,config=?2 WHERE id=?3 AND collection_id=?4",&[text(&view.config.name),text(&encoded),text(&view.id),text(&id)]).await.map_err(|e|unique_message(e,"a view with that name already exists"))?;if changed==0{return Err(AppError::NotFound);}}
        else {conn.execute("INSERT INTO collection_views(id,collection_id,name,config) VALUES(?1,?2,?3,?4)",&[text(&view.id),text(&id),text(&view.config.name),text(&encoded)]).await.map_err(|e|unique_message(e,"a view with that name already exists"))?;}
        Ok(view)
    }).await
}
pub async fn create_view(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path(id): Path<String>,
    Json(config): Json<SavedViewWrite>,
) -> Result<(StatusCode, Json<SavedView>), AppError> {
    Ok((
        StatusCode::CREATED,
        Json(write_view(state, principal, id, None, config).await?),
    ))
}
pub async fn update_view(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path((id, view_id)): Path<(String, String)>,
    Json(config): Json<SavedViewWrite>,
) -> Result<Json<SavedView>, AppError> {
    Ok(Json(
        write_view(state, principal, id, Some(view_id), config).await?,
    ))
}
pub async fn delete_view(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path((id, view_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let conn = state.db.connect().await?;
    workspace_collection(&conn, &principal, &id).await?;
    if conn
        .execute(
            "DELETE FROM collection_views WHERE id=?1 AND collection_id=?2",
            &[text(&view_id), text(&id)],
        )
        .await?
        == 0
    {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Points saved views' filters, sorting, grouping and columns at renamed fields.
pub async fn rename_view_fields(
    conn: &Conn,
    collection: &str,
    renames: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let rename = |name: &mut String| {
        if let Some(next) = renames.get(name.as_str()) {
            *name = next.clone();
        }
    };
    for row in conn
        .query(
            "SELECT id,config FROM collection_views WHERE collection_id=?1",
            &[text(collection)],
        )
        .await?
    {
        let mut config: SavedViewWrite = serde_json::from_str(&text_at(&row, 1)?)?;
        let original = config.clone();
        for filter in &mut config.query.filters {
            rename(&mut filter.field);
        }
        rename(&mut config.query.sort_field);
        rename(&mut config.query.group_field);
        config.columns.iter_mut().for_each(rename);
        if config != original {
            conn.execute(
                "UPDATE collection_views SET config=?1 WHERE id=?2",
                &[
                    text(&serde_json::to_string(&config)?),
                    text(&text_at(&row, 0)?),
                ],
            )
            .await?;
        }
    }
    Ok(())
}
