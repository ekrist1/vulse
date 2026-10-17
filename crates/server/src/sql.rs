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
    Blueprint, CollectionView, DocumentView, FieldDef, FieldType, MAX_SQL_CHANGES, MAX_SQL_SAMPLE,
    Role, SQL_RECORD_COLUMNS, SqlChange, SqlExpr, SqlRequest, SqlResult, SqlRow, SqlStatement,
    parse_sql, record_title, run_select, sql_assignments, sql_matches,
};

use crate::auth::{AuthUser, Principal, require_collection, require_role};
use crate::content::{check_body, materialize};
use crate::db::{Conn, SqlValue, opt_text_at, text_at};
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
    if matches!(statement, SqlStatement::Select(_)) {
        return Ok(Json(select(&conn, collection, &statement).await?));
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

/// Runs `sql` if it is a `SELECT`, and refuses anything that would change
/// records.
pub(crate) async fn select_only(
    conn: &Conn,
    principal: &Principal,
    sql: &str,
) -> Result<SqlResult, AppError> {
    let statement = parse_sql(sql).map_err(|e| AppError::bad(e.to_string()))?;
    if !matches!(statement, SqlStatement::Select(_)) {
        return Err(AppError::bad(
            "Only SELECT statements can run here. Use update_records or delete_records to change records.",
        ));
    }
    let collection = target(conn, principal, &statement).await?;
    select(conn, collection, &statement).await
}

async fn select(
    conn: &Conn,
    collection: CollectionView,
    statement: &SqlStatement,
) -> Result<SqlResult, AppError> {
    let SqlStatement::Select(select) = statement else {
        return Err(AppError::bad("not a SELECT statement"));
    };
    let records = records(conn, &collection, statement).await?;
    let mut fields = vec!["id".to_string()];
    fields.extend(collection.blueprint.fields.iter().map(|f| f.name.clone()));
    let (columns, rows, total) = run_select(select, &fields, &records).map_err(AppError::bad)?;
    let wanted = (total as usize)
        .saturating_sub(select.offset)
        .min(select.limit.unwrap_or(usize::MAX));
    Ok(SqlResult {
        kind: "select".into(),
        collection_id: collection.id,
        collection: collection.name,
        truncated: rows.len() < wanted,
        columns,
        rows,
        total,
        ..SqlResult::default()
    })
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

/// The records a `SELECT` may match, as SQL rows with the fields it reads.
async fn records(
    conn: &Conn,
    collection: &CollectionView,
    statement: &SqlStatement,
) -> Result<Vec<SqlRow>, AppError> {
    let blueprint = &collection.blueprint;
    let read: Vec<&FieldDef> = match statement {
        SqlStatement::Select(select) if select.columns.is_empty() => {
            blueprint.fields.iter().collect()
        }
        _ => {
            let names = statement.fields();
            blueprint
                .fields
                .iter()
                .filter(|f| names.contains(&f.name))
                .collect()
        }
    };
    // Values read from other collections are only current when calculated
    // now, so those statements read the record bodies.
    if read
        .iter()
        .any(|f| crate::lookups::reads_collections(blueprint, &f.name))
    {
        return Ok(documents(conn, collection, statement)
            .await?
            .iter()
            .map(row)
            .collect());
    }
    let (condition, params) = narrowing(blueprint, statement.filter());
    let mut columns = vec![
        "r.id".to_string(),
        "r.created_at".into(),
        "r.updated_at".into(),
    ];
    columns.extend(
        read.iter()
            .map(|f| format!("r.{}", crate::rows::column(&f.name, 'r'))),
    );
    let rows = conn
        .query(
            &format!(
                "SELECT {} FROM {} r WHERE {condition} ORDER BY r.created_at, r.id",
                columns.join(", "),
                crate::rows::table(&collection.id)
            ),
            &params,
        )
        .await?;
    rows.iter()
        .map(|values| {
            let mut out = SqlRow::new();
            for (index, name) in SQL_RECORD_COLUMNS.iter().enumerate() {
                out.insert(name.to_string(), Value::String(text_at(values, index)?));
            }
            for (offset, field) in read.iter().enumerate() {
                if let Some(raw) = opt_text_at(values, 3 + offset)? {
                    out.insert(field.name.clone(), serde_json::from_str(&raw)?);
                }
            }
            Ok(out)
        })
        .collect::<anyhow::Result<_>>()
        .map_err(Into::into)
}

/// The records a statement may match, with calculated values.
async fn documents(
    conn: &Conn,
    collection: &CollectionView,
    statement: &SqlStatement,
) -> Result<Vec<DocumentView>, AppError> {
    let (condition, params) = narrowing(&collection.blueprint, statement.filter());
    let rows = conn
        .query(
            &format!(
                "SELECT d.id, d.body, d.created_at, d.updated_at FROM {} r JOIN documents d ON d.id=r.id WHERE {condition} ORDER BY r.created_at, r.id",
                crate::rows::table(&collection.id)
            ),
            &params,
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

/// A condition on the record table (aliased `r`) that every record matching
/// `filter` meets, so only those records are read. It may let through more
/// records than match; the statement's own `WHERE` still decides. Parts it
/// cannot express are left out, which only widens it.
fn narrowing(blueprint: &Blueprint, filter: Option<&SqlExpr>) -> (String, Vec<SqlValue>) {
    let mut params = Vec::new();
    let condition = filter
        .and_then(|e| narrow(blueprint, e, &mut params))
        .unwrap_or_else(|| "1".into());
    (condition, params)
}

fn narrow(blueprint: &Blueprint, e: &SqlExpr, params: &mut Vec<SqlValue>) -> Option<String> {
    // A part that cannot be expressed takes back the values it bound.
    let bound = params.len();
    let condition = narrow_part(blueprint, e, params);
    if condition.is_none() {
        params.truncate(bound);
    }
    condition
}

fn narrow_part(blueprint: &Blueprint, e: &SqlExpr, params: &mut Vec<SqlValue>) -> Option<String> {
    let mut bind = |params: &mut Vec<SqlValue>, value: SqlValue| {
        params.push(value);
        format!("?{}", params.len())
    };
    match e {
        SqlExpr::Binary(op, a, b) if op == "AND" => {
            match (narrow(blueprint, a, params), narrow(blueprint, b, params)) {
                (Some(a), Some(b)) => Some(format!("({a} AND {b})")),
                (one, other) => one.or(other),
            }
        }
        SqlExpr::Binary(op, a, b) if op == "OR" => {
            let a = narrow(blueprint, a, params)?;
            let b = narrow(blueprint, b, params)?;
            Some(format!("({a} OR {b})"))
        }
        SqlExpr::Binary(op, a, b) => match (a.as_ref(), b.as_ref()) {
            (SqlExpr::Field(f), SqlExpr::Literal(v)) => {
                compare(blueprint, f, op, v, params, &mut bind)
            }
            (SqlExpr::Literal(v), SqlExpr::Field(f)) => {
                compare(blueprint, f, flipped(op), v, params, &mut bind)
            }
            _ => None,
        },
        SqlExpr::IsNull(a, not) => {
            let SqlExpr::Field(f) = a.as_ref() else {
                return None;
            };
            let text = match column(blueprint, f)? {
                Column::Record(_) if *not => return None,
                Column::Record(_) => return Some("0".into()),
                Column::Field(field) => field.text,
            };
            // Blank text counts as empty too.
            Some(if *not {
                format!("{text} IS NOT NULL")
            } else {
                format!("({text} IS NULL OR {text}='')")
            })
        }
        SqlExpr::In(a, list, false) => {
            let SqlExpr::Field(f) = a.as_ref() else {
                return None;
            };
            let parts = list
                .iter()
                .map(|item| match item {
                    SqlExpr::Literal(v) => compare(blueprint, f, "=", v, params, &mut bind),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?;
            (!parts.is_empty()).then(|| format!("({})", parts.join(" OR ")))
        }
        SqlExpr::Between(a, low, high, false) => narrow(
            blueprint,
            &SqlExpr::Binary(
                "AND".into(),
                Box::new(SqlExpr::Binary(">=".into(), a.clone(), low.clone())),
                Box::new(SqlExpr::Binary("<=".into(), a.clone(), high.clone())),
            ),
            params,
        ),
        SqlExpr::Like(a, pattern, false) => {
            let (SqlExpr::Field(f), SqlExpr::Literal(Value::String(pattern))) =
                (a.as_ref(), pattern.as_ref())
            else {
                return None;
            };
            let target = match column(blueprint, f)? {
                Column::Record(c) => c,
                Column::Field(field) if field.plain_text => field.text,
                Column::Field(_) => return None,
            };
            let p = bind(params, SqlValue::Text(pattern.to_lowercase()));
            Some(format!("{target} LIKE {p}"))
        }
        _ => None,
    }
}

enum Column {
    /// `id`, `created_at` or `updated_at`: always set, compared as stored.
    Record(String),
    Field(FieldColumns),
}

struct FieldColumns {
    text: String,
    number: String,
    numeric: bool,
    /// Whether values compare as their lowercase text column: plain text
    /// whose display is the value itself.
    plain_text: bool,
    date: bool,
}

fn column(blueprint: &Blueprint, name: &str) -> Option<Column> {
    if SQL_RECORD_COLUMNS.contains(&name) {
        return Some(Column::Record(format!("r.{name}")));
    }
    let field = blueprint.fields.iter().find(|f| f.name == name)?;
    Some(Column::Field(FieldColumns {
        text: format!("r.{}", crate::rows::column(name, 't')),
        number: format!("r.{}", crate::rows::column(name, 'n')),
        numeric: field.kind == FieldType::Number,
        plain_text: matches!(
            field.kind,
            FieldType::String | FieldType::Date | FieldType::Reference | FieldType::Asset
        ),
        date: field.kind == FieldType::Date,
    }))
}

/// The operator with its sides swapped: `5 < x` is `x > 5`.
fn flipped(op: &str) -> &str {
    match op {
        "<" => ">",
        ">" => "<",
        "<=" => ">=",
        ">=" => "<=",
        op => op,
    }
}

/// `field op literal`. Vulse SQL compares numbers, and text that reads as
/// a number, by value, and anything else as case-sensitive text.
fn compare(
    blueprint: &Blueprint,
    field: &str,
    op: &str,
    literal: &Value,
    params: &mut Vec<SqlValue>,
    bind: &mut impl FnMut(&mut Vec<SqlValue>, SqlValue) -> String,
) -> Option<String> {
    if !["=", "<>", "<", ">", "<=", ">="].contains(&op) {
        return None;
    }
    let number = match literal {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => return None,
    };
    let text = match literal {
        Value::String(s) => s.clone(),
        _ => literal.to_string(),
    };
    match column(blueprint, field)? {
        // Stored ids and timestamps never read as numbers.
        Column::Record(_) if number.is_some() => None,
        Column::Record(_) if op == "<>" => None,
        Column::Record(c) => {
            let p = bind(params, SqlValue::Text(text));
            Some(format!("{c}{op}{p}"))
        }
        Column::Field(f) => match number {
            Some(n) if f.numeric => {
                let p = bind(params, SqlValue::Real(n));
                Some(format!("{}{op}{p}", f.number))
            }
            // Text values compare as text against a number, so they pass.
            Some(n) => {
                let p = bind(params, SqlValue::Real(n));
                Some(format!(
                    "({}{op}{p} OR ({} IS NULL AND {} IS NOT NULL))",
                    f.number, f.number, f.text
                ))
            }
            None if op == "=" && f.plain_text => {
                let p = bind(params, SqlValue::Text(text.to_lowercase()));
                Some(format!("{}={p}", f.text))
            }
            // ISO dates order the same in either case.
            None if op != "<>" && f.date => {
                let p = bind(params, SqlValue::Text(text.to_lowercase()));
                Some(format!("{}{op}{p}", f.text))
            }
            None => None,
        },
    }
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
    for doc in documents(conn, collection, statement).await? {
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
