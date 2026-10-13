//! `POST /sql`: Vulse SQL statements over a collection's records.
//!
//! `SELECT` returns rows. `UPDATE` and `DELETE` first return a preview of
//! the changes with a confirmation; sending that confirmation back applies
//! exactly those changes, through the normal record path (validation,
//! history, Trash and formulas), in one transaction.

use axum::Json;
use axum::extract::State;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use vulse_model::{
    CollectionView, DocumentView, FieldType, MAX_SQL_CHANGES, MAX_SQL_SAMPLE, Role,
    SQL_RECORD_COLUMNS, SqlChange, SqlRequest, SqlResult, SqlRow, SqlStatement, parse_sql,
    record_title, run_select, sql_assignments, sql_matches,
};

use crate::auth::{AuthUser, Principal, require_collection, require_role};
use crate::content::{check_body, materialize};
use crate::db::{Conn, SqlValue};
use crate::error::AppError;
use crate::state::AppState;
use crate::store;

/// A previewed change to one record.
struct Planned {
    doc: DocumentView,
    title: String,
    /// The new stored body; `None` deletes the record.
    body: Option<Value>,
    before: Map<String, Value>,
    after: Option<Map<String, Value>>,
}

pub async fn run_sql(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Json(request): Json<SqlRequest>,
) -> Result<Json<SqlResult>, AppError> {
    let statement = parse_sql(&request.sql).map_err(|e| AppError::bad(e.to_string()))?;
    let conn = state.db.connect().await?;
    let collection = target(&conn, &principal, &statement).await?;
    if let SqlStatement::Select(select) = &statement {
        let records = records(&conn, &collection).await?;
        let mut fields = vec!["id".to_string()];
        fields.extend(collection.blueprint.fields.iter().map(|f| f.name.clone()));
        let (columns, rows, total) =
            run_select(select, &fields, &records).map_err(AppError::bad)?;
        let wanted = (total as usize)
            .saturating_sub(select.offset)
            .min(select.limit.unwrap_or(usize::MAX));
        return Ok(Json(SqlResult {
            kind: "select".into(),
            collection_id: collection.id,
            collection: collection.name,
            truncated: rows.len() < wanted,
            columns,
            rows,
            total,
            ..SqlResult::default()
        }));
    }
    require_role(&principal, Role::Editor)?;
    let sql = request.sql.trim().to_string();
    let Some(confirm) = request.confirm else {
        let plan = plan(&conn, &collection, &statement).await?;
        let preview = result(&statement, &collection, &sql, &plan, false);
        return Ok(Json(
            with_problems(&conn, &collection, &plan, preview).await?,
        ));
    };
    let applied = conn
        .with_app_tx(async |conn| {
            // Planned again inside the transaction, so nothing changes in between.
            let collection = target(conn, &principal, &statement).await?;
            let plan = plan(conn, &collection, &statement).await?;
            let preview = result(&statement, &collection, &sql, &plan, false);
            if preview.confirmation.as_deref() != Some(confirm.as_str()) {
                return Err(AppError::conflict(
                    "The matching records changed since the preview. Review the new preview before running the statement.",
                ));
            }
            let problems = problems(conn, &collection, &plan).await?;
            if !problems.is_empty() {
                return Err(AppError::bad(problems.join(" ")));
            }
            let now = crate::timeutil::now_rfc3339();
            for change in &plan {
                match &change.body {
                    Some(body) => {
                        store::update_document_as(conn, &collection.id, &change.doc.id, body, &now, "sql update").await?;
                    }
                    None => {
                        store::delete_document(conn, &collection.id, &change.doc.id).await?;
                    }
                }
            }
            let mut done = result(&statement, &collection, &sql, &plan, true);
            done.confirmation = None;
            Ok(done)
        })
        .await?;
    Ok(Json(applied))
}

/// The statement's collection, checked for access and unknown fields.
async fn target(
    conn: &Conn,
    principal: &Principal,
    statement: &SqlStatement,
) -> Result<CollectionView, AppError> {
    let name = statement.table();
    let collection = store::find_collection_by_name(conn, name)
        .await?
        .ok_or_else(|| AppError::bad(format!("There is no collection named '{name}'.")))?;
    require_collection(principal, &collection.id)?;
    let fields = &collection.blueprint.fields;
    for field in statement.fields() {
        if !SQL_RECORD_COLUMNS.contains(&field.as_str()) && !fields.iter().any(|f| f.name == field)
        {
            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            return Err(AppError::bad(format!(
                "Collection '{name}' has no field '{field}'. Its fields are: {}.",
                names.join(", ")
            )));
        }
    }
    if let SqlStatement::Update { set, .. } = statement {
        for (field, _) in set {
            if SQL_RECORD_COLUMNS.contains(&field.as_str()) {
                return Err(AppError::bad(format!(
                    "'{field}' is set by Vulse and cannot be changed."
                )));
            }
            if fields
                .iter()
                .any(|f| f.name == *field && f.kind == FieldType::Calculated)
            {
                return Err(AppError::bad(format!(
                    "'{field}' is calculated from its formula; change its input fields instead."
                )));
            }
        }
    }
    Ok(collection)
}

/// Every record, with calculated values, as SQL rows.
async fn records(conn: &Conn, collection: &CollectionView) -> Result<Vec<SqlRow>, AppError> {
    Ok(documents(conn, collection)
        .await?
        .iter()
        .map(row)
        .collect())
}

async fn documents(
    conn: &Conn,
    collection: &CollectionView,
) -> Result<Vec<DocumentView>, AppError> {
    let rows = conn
        .query(
            "SELECT id, body, created_at, updated_at FROM documents WHERE collection_id=?1 ORDER BY created_at, id",
            &[SqlValue::Text(collection.id.clone())],
        )
        .await?;
    let lookups = crate::lookups::load(conn, &collection.blueprint).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut doc = store::document_from_row(row)?;
        materialize(&collection.blueprint, &lookups, &mut doc);
        out.push(doc);
    }
    Ok(out)
}

fn row(doc: &DocumentView) -> SqlRow {
    let mut row = doc.body.as_object().cloned().unwrap_or_default();
    row.insert("id".into(), Value::String(doc.id.clone()));
    row.insert("created_at".into(), Value::String(doc.created_at.clone()));
    row.insert("updated_at".into(), Value::String(doc.updated_at.clone()));
    row
}

/// The records an `UPDATE` or `DELETE` changes, and how.
async fn plan(
    conn: &Conn,
    collection: &CollectionView,
    statement: &SqlStatement,
) -> Result<Vec<Planned>, AppError> {
    let blueprint = &collection.blueprint;
    let mut plan = Vec::new();
    for doc in documents(conn, collection).await? {
        let row = row(&doc);
        if !sql_matches(statement.filter(), &row).map_err(AppError::bad)? {
            continue;
        }
        let title = record_title(blueprint, &doc.body, &doc.id);
        let mut before = Map::new();
        let planned = match statement {
            SqlStatement::Update { set, .. } => {
                let values = sql_assignments(set, &row)
                    .map_err(|e| AppError::bad(format!("Record '{title}': {e}")))?;
                let mut body = vulse_model::source_document(blueprint, &doc.body);
                let object = body
                    .as_object_mut()
                    .ok_or_else(|| anyhow::anyhow!("record is not an object"))?;
                let mut after = Map::new();
                for (field, value) in values {
                    let current = object.get(&field).cloned().unwrap_or(Value::Null);
                    // Blank text counts as empty, as in the record editor.
                    let value = if value == Value::String(String::new()) {
                        Value::Null
                    } else {
                        value
                    };
                    if current == value {
                        continue;
                    }
                    before.insert(field.clone(), current);
                    after.insert(field.clone(), value.clone());
                    if value.is_null() {
                        object.remove(&field);
                    } else {
                        object.insert(field, value);
                    }
                }
                if after.is_empty() {
                    continue;
                }
                Planned {
                    doc,
                    title,
                    body: Some(body),
                    before,
                    after: Some(after),
                }
            }
            _ => {
                before = doc.body.as_object().cloned().unwrap_or_default();
                Planned {
                    doc,
                    title,
                    body: None,
                    before,
                    after: None,
                }
            }
        };
        plan.push(planned);
        if plan.len() > MAX_SQL_CHANGES {
            return Err(AppError::bad(format!(
                "This statement changes more than {MAX_SQL_CHANGES} records. Narrow it with WHERE."
            )));
        }
    }
    Ok(plan)
}

/// Checks planned changes; problems block running the statement.
async fn problems(
    conn: &Conn,
    collection: &CollectionView,
    plan: &[Planned],
) -> Result<Vec<String>, AppError> {
    let mut problems = Vec::new();
    let deletions: Vec<String> = plan
        .iter()
        .filter(|p| p.body.is_none())
        .map(|p| p.doc.id.clone())
        .collect();
    if !deletions.is_empty() {
        match crate::relationships::guard_delete(conn, &collection.id, &deletions, "document").await
        {
            Ok(()) => {}
            Err(AppError::Conflict {
                message,
                document_ids,
            }) => problems.push(format!(
                "{message}: {} record{} still link to them.",
                document_ids.len(),
                if document_ids.len() == 1 { "" } else { "s" }
            )),
            Err(e) => return Err(e),
        }
    }
    for change in plan {
        let Some(body) = &change.body else { continue };
        match check_body(
            conn,
            &collection.id,
            &collection.name,
            &collection.blueprint,
            body,
        )
        .await
        {
            Ok(()) => {}
            Err(AppError::BadRequest(message)) => {
                problems.push(format!(
                    "Record '{}' ({}): {message}",
                    change.title, change.doc.id
                ));
                if problems.len() >= MAX_SQL_SAMPLE {
                    break;
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(problems)
}

fn result(
    statement: &SqlStatement,
    collection: &CollectionView,
    sql: &str,
    plan: &[Planned],
    done: bool,
) -> SqlResult {
    // The confirmation covers the statement and the exact record versions.
    let mut hash = Sha256::new();
    hash.update(sql.as_bytes());
    for change in plan {
        hash.update(b"\n");
        hash.update(change.doc.id.as_bytes());
        hash.update(b"@");
        hash.update(change.doc.updated_at.as_bytes());
    }
    SqlResult {
        kind: statement.kind().into(),
        collection_id: collection.id.clone(),
        collection: collection.name.clone(),
        affected: plan.len() as u64,
        changes: plan
            .iter()
            .take(MAX_SQL_SAMPLE)
            .map(|p| SqlChange {
                id: p.doc.id.clone(),
                title: p.title.clone(),
                before: p.before.clone(),
                after: p.after.clone(),
            })
            .collect(),
        confirmation: (!plan.is_empty()).then(|| hex::encode(&hash.finalize()[..16])),
        done,
        ..SqlResult::default()
    }
}

/// Adds problems to a preview; a statement with problems cannot run.
async fn with_problems(
    conn: &Conn,
    collection: &CollectionView,
    plan: &[Planned],
    mut result: SqlResult,
) -> Result<SqlResult, AppError> {
    result.problems = problems(conn, collection, plan).await?;
    if !result.problems.is_empty() {
        result.confirmation = None;
    }
    Ok(result)
}
