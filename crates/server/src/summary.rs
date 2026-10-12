//! Grouped summaries: `POST /collections/{id}/summary`, a saved view's
//! summary, and the public variant for public collections.

use std::cmp::Ordering;
use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Map, Value};
use vulse_model::{
    CollectionView, FieldType, MAX_SUMMARY_GROUPS, SavedViewWrite, SummaryOp, SummaryRequest,
    SummaryResult, SummaryRow, SummarySpec, apply_summary_formulas, validate_summary,
};

use crate::auth::AuthUser;
use crate::content::workspace_collection;
use crate::db::{Conn, SqlRow, SqlValue, int_at, opt_number_at, text_at};
use crate::error::AppError;
use crate::state::AppState;
use crate::store;

/// Groups computed before refusing; only `MAX_SUMMARY_GROUPS` are returned.
const MAX_COMPUTED_GROUPS: usize = 10_000;

pub async fn summarize(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path(id): Path<String>,
    Json(request): Json<SummaryRequest>,
) -> Result<Json<SummaryResult>, AppError> {
    let conn = state.db.connect().await?;
    let collection = workspace_collection(&conn, &principal, &id).await?;
    Ok(Json(run_summary(&conn, &collection, &request).await?))
}

/// Runs the summary saved in a view, with the view's search and filters.
pub async fn view_summary(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path((id, view_id)): Path<(String, String)>,
) -> Result<Json<SummaryResult>, AppError> {
    let conn = state.db.connect().await?;
    let collection = workspace_collection(&conn, &principal, &id).await?;
    let rows = conn
        .query(
            "SELECT config FROM collection_views WHERE id=?1 AND collection_id=?2",
            &[SqlValue::Text(view_id), SqlValue::Text(id)],
        )
        .await?;
    let row = rows.first().ok_or(AppError::NotFound)?;
    let config: SavedViewWrite =
        serde_json::from_str(&text_at(row, 0)?).map_err(anyhow::Error::from)?;
    let summary = config
        .summary
        .ok_or_else(|| AppError::bad("this view has no summary"))?;
    let request = SummaryRequest {
        query: config.query,
        summary,
    };
    Ok(Json(run_summary(&conn, &collection, &request).await?))
}

pub(crate) async fn run_summary(
    conn: &Conn,
    collection: &CollectionView,
    request: &SummaryRequest,
) -> Result<SummaryResult, AppError> {
    let spec = &request.summary;
    let blueprint = &collection.blueprint;
    validate_summary(blueprint, spec).map_err(AppError::bad)?;
    // Only the search and filters matter; grouping replaces sorting and paging.
    let mut query = request.query.clone();
    query.sort_field = "created_at".into();
    query.group_field = String::new();
    query.offset = 0;
    let (from, _, mut params) = crate::records::query_sql(blueprint, &collection.id, &query)?;
    let filters = from
        .strip_prefix("FROM documents d")
        .ok_or_else(|| anyhow::anyhow!("unexpected query shape"))?
        .to_string();

    let mut joins = String::new();
    let mut aliases = HashMap::<String, String>::new();
    let mut join = |field: &str, params: &mut Vec<SqlValue>| -> String {
        if let Some(alias) = aliases.get(field) {
            return alias.clone();
        }
        params.push(SqlValue::Text(field.to_string()));
        let alias = format!("c{}", aliases.len());
        joins.push_str(&format!(
            " LEFT JOIN document_cells {alias} ON {alias}.document_id=d.id AND {alias}.field=?{}",
            params.len()
        ));
        aliases.insert(field.to_string(), alias.clone());
        alias
    };
    let kind = |name: &str| {
        blueprint
            .fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.kind)
    };

    // Group keys are stored JSON values; date buckets are plain text prefixes.
    let keys: Vec<String> = spec
        .group_by
        .iter()
        .map(|group| {
            let alias = join(&group.field, &mut params);
            match group.bucket {
                Some(bucket) => format!(
                    "CASE WHEN {alias}.text_value<>'' THEN substr({alias}.text_value,1,{}) END",
                    bucket.width()
                ),
                None => format!("{alias}.raw_value"),
            }
        })
        .collect();
    // One SQL column per aggregate measure; formulas are evaluated afterwards.
    let mut columns = Vec::new();
    for measure in &spec.measures {
        let Some(field) = measure.field.as_deref() else {
            continue;
        };
        let alias = join(field, &mut params);
        let number = format!("CAST({alias}.number_value AS REAL)");
        let date = kind(field) == Some(FieldType::Date);
        let expression = match measure.op {
            SummaryOp::Sum => format!("SUM({number})"),
            SummaryOp::Average => format!("AVG({number})"),
            SummaryOp::Min if date => format!("MIN(NULLIF({alias}.text_value,''))"),
            SummaryOp::Max if date => format!("MAX(NULLIF({alias}.text_value,''))"),
            SummaryOp::Min => format!("MIN({number})"),
            SummaryOp::Max => format!("MAX({number})"),
            SummaryOp::Count => format!("COUNT({alias}.document_id)"),
            SummaryOp::CountUnique => format!("COUNT(DISTINCT {alias}.raw_value)"),
            SummaryOp::Formula => continue,
        };
        columns.push((measure.name.clone(), expression));
    }
    let select = |keys: &[String]| {
        let mut parts = keys.to_vec();
        parts.push("COUNT(*)".into());
        parts.extend(columns.iter().map(|(_, expression)| expression.clone()));
        parts.join(",")
    };

    let totals_sql = format!("SELECT {} FROM documents d{joins}{filters}", select(&[]));
    let totals = conn.query(&totals_sql, &params).await?;
    let totals = decode_row(
        spec,
        &columns,
        &[],
        totals.first().ok_or(AppError::NotFound)?,
        0,
    )?;

    let mut rows = Vec::new();
    if keys.is_empty() {
        rows.push(totals.clone());
    } else {
        let sql = format!(
            "SELECT {} FROM documents d{joins}{filters} GROUP BY {} LIMIT {}",
            select(&keys),
            keys.join(","),
            MAX_COMPUTED_GROUPS + 1
        );
        let found = conn.query(&sql, &params).await?;
        if found.len() > MAX_COMPUTED_GROUPS {
            return Err(AppError::bad(
                "This summary has more than 10,000 groups. Group by fewer fields, use a coarser date grouping, or filter the records.",
            ));
        }
        for row in &found {
            rows.push(decode_row(spec, &columns, &spec.group_by, row, keys.len())?);
        }
    }
    label_groups(conn, collection, spec, &mut rows).await?;
    sort_rows(spec, &mut rows);
    let group_count = rows.len() as u64;
    let truncated = rows.len() > MAX_SUMMARY_GROUPS;
    rows.truncate(MAX_SUMMARY_GROUPS);
    Ok(SummaryResult {
        rows,
        totals,
        group_count,
        truncated,
    })
}

fn decode_row(
    spec: &SummarySpec,
    columns: &[(String, String)],
    groups: &[vulse_model::SummaryGroup],
    row: &SqlRow,
    key_count: usize,
) -> Result<SummaryRow, AppError> {
    let group = groups
        .iter()
        .enumerate()
        .map(|(index, group)| match row.get(index) {
            Some(SqlValue::Text(text)) if group.bucket.is_some() => Value::String(text.clone()),
            Some(SqlValue::Text(raw)) => {
                serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone()))
            }
            _ => Value::Null,
        })
        .collect::<Vec<_>>();
    let count = int_at(row, key_count)? as u64;
    let mut values = Map::new();
    for (offset, (name, _)) in columns.iter().enumerate() {
        let index = key_count + 1 + offset;
        let value = match row.get(index) {
            Some(SqlValue::Text(text)) => Value::String(text.clone()),
            _ => opt_number_at(row, index)?
                .map(number)
                .unwrap_or(Value::Null),
        };
        values.insert(name.clone(), value);
    }
    // A count without a field counts the group's records.
    for measure in &spec.measures {
        if measure.op == SummaryOp::Count && measure.field.is_none() {
            values.insert(measure.name.clone(), Value::from(count));
        }
    }
    let mut row = SummaryRow {
        labels: Vec::new(),
        group,
        count,
        values,
        errors: Default::default(),
    };
    apply_summary_formulas(spec, &mut row);
    Ok(row)
}

/// Whole numbers as integers, so totals read `3000` rather than `3000.0`.
fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        Value::from(value as i64)
    } else {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// Readable group values; references show the related record's title.
async fn label_groups(
    conn: &Conn,
    collection: &CollectionView,
    spec: &SummarySpec,
    rows: &mut [SummaryRow],
) -> anyhow::Result<()> {
    let mut titles = HashMap::<(String, String), String>::new();
    for (index, group) in spec.group_by.iter().enumerate() {
        let field = collection
            .blueprint
            .fields
            .iter()
            .find(|f| f.name == group.field);
        let target = match field.filter(|f| f.kind == FieldType::Reference) {
            Some(field) => {
                let name = field.collection.as_deref().unwrap_or_default();
                if name == collection.name {
                    Some(collection.clone())
                } else {
                    store::find_collection_by_name(conn, name).await?
                }
            }
            None => None,
        };
        for row in rows.iter_mut() {
            let value = &row.group[index];
            let label = match (value, &target) {
                (Value::Null, _) => "(empty)".to_string(),
                (Value::String(id), Some(target)) => {
                    let key = (target.id.clone(), id.clone());
                    if !titles.contains_key(&key) {
                        let title = store::find_document(conn, &target.id, id)
                            .await?
                            .map(|mut doc| {
                                crate::content::materialize(&target.blueprint, &mut doc);
                                vulse_model::record_title(&target.blueprint, &doc.body, &doc.id)
                            })
                            .unwrap_or_else(|| format!("Missing record {id}"));
                        titles.insert(key.clone(), title);
                    }
                    titles[&key].clone()
                }
                (value, _) => vulse_model::display_value(value),
            };
            row.labels.push(label);
        }
    }
    Ok(())
}

/// Orders groups by the chosen column, or by the group values. Empty values
/// always come last.
fn sort_rows(spec: &SummarySpec, rows: &mut [SummaryRow]) {
    let measure = spec
        .sort
        .as_ref()
        .filter(|name| spec.measures.iter().any(|m| m.name == **name));
    let group = spec
        .sort
        .as_ref()
        .and_then(|name| spec.group_by.iter().position(|g| g.field == *name));
    rows.sort_by(|a, b| {
        let ordering = if let Some(name) = measure {
            compare(&a.values[name], &b.values[name], spec.descending)
        } else if let Some(index) = group {
            compare(&a.group[index], &b.group[index], spec.descending)
        } else {
            a.group
                .iter()
                .zip(&b.group)
                .map(|(x, y)| compare(x, y, spec.descending))
                .find(|o| o.is_ne())
                .unwrap_or(Ordering::Equal)
        };
        ordering.then_with(|| a.labels.cmp(&b.labels))
    });
}

fn compare(a: &Value, b: &Value, descending: bool) -> Ordering {
    let ordering = match (a, b) {
        (Value::Null, Value::Null) => return Ordering::Equal,
        (Value::Null, _) => return Ordering::Greater,
        (_, Value::Null) => return Ordering::Less,
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .partial_cmp(&y.as_f64())
            .unwrap_or(Ordering::Equal),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ => vulse_model::display_value(a)
            .to_lowercase()
            .cmp(&vulse_model::display_value(b).to_lowercase()),
    };
    if descending {
        ordering.reverse()
    } else {
        ordering
    }
}
