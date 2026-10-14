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

pub async fn sync_collection_cells(
    conn: &Conn,
    id: &str,
    blueprint: &Blueprint,
) -> anyhow::Result<()> {
    let lookups = crate::lookups::load(conn, blueprint).await?;
    crate::rows::rebuild(conn, id, blueprint, &lookups).await?;
    if !crate::relationships::has_links(blueprint) {
        // Nothing to link from; drop links of fields that were removed.
        conn.execute(
            "DELETE FROM document_links WHERE source_id IN (SELECT id FROM documents WHERE collection_id=?1)",
            &[text(id)],
        )
        .await?;
        return Ok(());
    }
    for (document, body) in store::list_document_bodies(conn, id).await? {
        crate::relationships::sync(conn, id, &document, blueprint, &body).await?;
    }
    Ok(())
}
/// Rebuilds every collection's record table from the record bodies.
pub async fn rebuild_all_tables(conn: &Conn) -> anyhow::Result<()> {
    for row in conn
        .query("SELECT id,blueprint FROM collections", &[])
        .await?
    {
        let blueprint = serde_json::from_str(&text_at(&row, 1)?)?;
        let lookups = crate::lookups::load(conn, &blueprint).await?;
        crate::rows::rebuild(conn, &text_at(&row, 0)?, &blueprint, &lookups).await?;
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
    let lookups = crate::lookups::load(conn, &blueprint).await?;
    crate::rows::sync(conn, collection, &blueprint, &lookups, id, body).await?;
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

/// Shared by empty/not-empty filters and counts of non-empty values.
pub(crate) fn nonempty_cell(field: &str) -> String {
    let (t, r) = (
        crate::rows::column(field, 't'),
        crate::rows::column(field, 'r'),
    );
    format!("r.{t}<>'' AND r.{r}<>'[]' AND r.{r}<>'{{}}'")
}

/// A record query over a collection's record table, aliased `r`.
pub(crate) struct QueryPlan {
    pub table: String,
    /// The `WHERE` condition.
    pub filter: String,
    pub params: Vec<SqlValue>,
    /// A complete `ORDER BY`, with empty values last.
    pub order: String,
    /// The sorted column and direction when no grouping applies; pages then
    /// read it through its index.
    sort: Option<(String, bool)>,
}

pub(crate) fn query_sql(
    blueprint: &Blueprint,
    id: &str,
    query: &RecordQuery,
) -> Result<QueryPlan, AppError> {
    use crate::rows::column;
    validate_query(blueprint, query).map_err(AppError::bad)?;
    let mut params = vec![];
    let mut conditions = vec![];
    if !query.search.trim().is_empty() {
        let p = parameter(&mut params, text(&like(query.search.trim())));
        conditions.push(format!(
            "(lower(r.id) LIKE {p} ESCAPE '\\' OR r.search LIKE {p} ESCAPE '\\')"
        ));
    }
    // Filters joined by OR share a group; the groups are joined by AND.
    let mut groups: Vec<Vec<String>> = vec![];
    for filter in &query.filters {
        let is_date = blueprint
            .fields
            .iter()
            .any(|f| f.name == filter.field && f.kind == FieldType::Date);
        let t = format!("r.{}", column(&filter.field, 't'));
        let n = format!("r.{}", column(&filter.field, 'n'));
        let predicate = if ["empty", "not_empty"].contains(&filter.op.as_str()) {
            nonempty_cell(&filter.field)
        } else {
            let value = if filter.op == "contains" {
                like(&vulse_model::display_value(&filter.value))
            } else {
                vulse_model::display_value(&filter.value).to_lowercase()
            };
            let p = parameter(&mut params, text(&value));
            match filter.op.as_str() {
                "contains" => format!("{t} LIKE {p} ESCAPE '\\'"),
                "eq" | "neq" if filter.value.is_number() => format!("{n}=CAST({p} AS REAL)"),
                "eq" | "neq" => format!("{t}={p}"),
                // ISO dates order correctly as text.
                op if is_date => format!(
                    "{t}<>'' AND {t}{}{p}",
                    match op {
                        "gt" => ">",
                        "gte" => ">=",
                        "lt" => "<",
                        _ => "<=",
                    }
                ),
                op => format!(
                    "{n}{}CAST({p} AS REAL)",
                    match op {
                        "gt" => ">",
                        "gte" => ">=",
                        "lt" => "<",
                        _ => "<=",
                    }
                ),
            }
        };
        // Records without a value match neither a condition nor its opposite,
        // so negations count a missing value as not matching.
        let condition = if matches!(filter.op.as_str(), "empty" | "neq") {
            format!("NOT COALESCE(({predicate}),0)")
        } else {
            format!("({predicate})")
        };
        match groups.last_mut() {
            Some(group) if filter.or => group.push(condition),
            _ => groups.push(vec![condition]),
        }
    }
    conditions.extend(groups.into_iter().map(|group| match group.as_slice() {
        [single] => single.clone(),
        _ => format!("({})", group.join(" OR ")),
    }));
    if conditions.is_empty() {
        conditions.push("1".into());
    }
    let direction = if query.descending { "DESC" } else { "ASC" };
    let sorted = if ["created_at", "updated_at", "id"].contains(&query.sort_field.as_str()) {
        format!("r.{}", query.sort_field)
    } else {
        let field = blueprint
            .fields
            .iter()
            .find(|f| f.name == query.sort_field)
            .ok_or_else(|| AppError::bad("unknown sort field"))?;
        format!("r.{}", crate::rows::sort_column(field))
    };
    let mut order = vec![];
    if !query.group_field.is_empty() {
        order.push(format!("r.{} ASC", column(&query.group_field, 't')));
    }
    order.push(format!("({sorted} IS NULL) ASC"));
    order.push(format!("{sorted} {direction}"));
    order.push(format!("r.id {direction}"));
    Ok(QueryPlan {
        table: crate::rows::table(id),
        filter: conditions.join(" AND "),
        params,
        order: order.join(","),
        sort: query
            .group_field
            .is_empty()
            .then_some((sorted, query.descending)),
    })
}
pub(crate) async fn run_query(
    conn: &Conn,
    collection: &vulse_model::CollectionView,
    query: &RecordQuery,
) -> Result<RecordPage, AppError> {
    let plan = query_sql(&collection.blueprint, &collection.id, query)?;
    let QueryPlan {
        table,
        filter,
        params,
        ..
    } = &plan;
    let count = conn
        .query(
            &format!("SELECT count(*) FROM {table} r WHERE {filter}"),
            params,
        )
        .await?;
    let total = int_at(&count[0], 0)? as u64;
    let ids = page_ids(conn, &plan, query.limit as i64, query.offset as i64).await?;
    let mut items = Vec::with_capacity(ids.len());
    if !ids.is_empty() {
        let slots: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let rows = conn
            .query(
                &format!(
                    "SELECT id,body,created_at,updated_at FROM documents WHERE id IN ({})",
                    slots.join(",")
                ),
                &ids.iter().map(|id| text(id)).collect::<Vec<_>>(),
            )
            .await?;
        let mut found = rows
            .iter()
            .map(store::document_from_row)
            .collect::<anyhow::Result<Vec<_>>>()?;
        for id in &ids {
            if let Some(index) = found.iter().position(|doc| doc.id == *id) {
                items.push(found.swap_remove(index));
            }
        }
    }
    let lookups = crate::lookups::load(conn, &collection.blueprint).await?;
    for doc in &mut items {
        materialize(&collection.blueprint, &lookups, doc);
    }
    Ok(RecordPage {
        items,
        total,
        limit: query.limit,
        offset: query.offset,
    })
}

/// The ids on one page. Without grouping, records with a value are read in
/// index order and records without one follow, so no page sorts the whole
/// collection.
async fn page_ids(
    conn: &Conn,
    plan: &QueryPlan,
    limit: i64,
    offset: i64,
) -> Result<Vec<String>, AppError> {
    let QueryPlan {
        table,
        filter,
        params,
        order,
        sort,
    } = plan;
    let page = async |condition: &str, order: &str, limit: i64, offset: i64| {
        let mut params = params.clone();
        let limit = parameter(&mut params, SqlValue::Integer(limit));
        let offset = parameter(&mut params, SqlValue::Integer(offset));
        let rows = conn
            .query(
                &format!(
                    "SELECT r.id FROM {table} r WHERE {filter}{condition} ORDER BY {order} LIMIT {limit} OFFSET {offset}"
                ),
                &params,
            )
            .await?;
        rows.iter()
            .map(|row| text_at(row, 0))
            .collect::<anyhow::Result<Vec<_>>>()
    };
    let Some((column, descending)) = sort else {
        return Ok(page("", order, limit, offset).await?);
    };
    if *descending {
        // Empty values sort lowest, so they already come last.
        return Ok(page("", &format!("{column} DESC, r.id DESC"), limit, offset).await?);
    }
    let mut ids = page(
        &format!(" AND {column} IS NOT NULL"),
        &format!("{column}, r.id"),
        limit,
        offset,
    )
    .await?;
    if (ids.len() as i64) < limit {
        let with_values = if ids.is_empty() {
            let rows = conn
                .query(
                    &format!(
                        "SELECT count(*) FROM {table} r WHERE {filter} AND {column} IS NOT NULL"
                    ),
                    params,
                )
                .await?;
            int_at(&rows[0], 0)?
        } else {
            offset + ids.len() as i64
        };
        ids.extend(
            page(
                &format!(" AND {column} IS NULL"),
                "r.id",
                limit - ids.len() as i64,
                (offset - with_values).max(0),
            )
            .await?,
        );
    }
    Ok(ids)
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
        if let Some(summary) = &mut config.summary {
            vulse_model::rename_summary_fields(summary, renames);
        }
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
