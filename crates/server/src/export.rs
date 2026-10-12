use crate::{
    auth::{AuthUser, Principal, require_collection},
    content::{materialize, workspace_collection},
    db::{Conn, SqlValue, int_at},
    error::AppError,
    state::AppState,
    store,
};
use axum::{
    Json,
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
};
use std::collections::HashMap;

use serde_json::{Map, Value};
use vulse_model::{
    ArrayItems, CollectionView, DocumentView, ExportRequest, FieldType, RelatedColumns,
};
const MAX_RECORDS: usize = 10_000;
const MAX_BYTES: usize = 25 * 1024 * 1024;
pub async fn export_records(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path(id): Path<String>,
    Json(write): Json<ExportRequest>,
) -> Result<Response, AppError> {
    if !["csv", "json"].contains(&write.format.as_str()) {
        return Err(AppError::bad("format must be csv or json"));
    }
    if write.ids.len() > 100
        || write
            .ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != write.ids.len()
    {
        return Err(AppError::bad("Select at most 100 unique records"));
    }
    let conn = state.db.connect().await?;
    let (name,bytes)=conn.with_app_tx(async |conn|{
 let collection=workspace_collection(conn,&principal,&id).await?;
 if write.columns.is_empty()||write.columns.iter().collect::<std::collections::HashSet<_>>().len()!=write.columns.len()||write.columns.iter().any(|key|!collection.blueprint.fields.iter().any(|f|f.name==*key)){return Err(AppError::bad("Choose one or more valid export columns"));}
 let(mut from,order,mut params)=crate::records::query_sql(&collection.blueprint,&id,&write.query)?;
 if !write.ids.is_empty(){
   // Selection is explicit and authorized separately from the current filter.
   from="FROM documents d WHERE d.collection_id=?1".into();params=vec![SqlValue::Text(id.clone())];
   let slots=write.ids.iter().map(|doc|{params.push(SqlValue::Text(doc.clone()));format!("?{}",params.len())}).collect::<Vec<_>>();from.push_str(&format!(" AND d.id IN ({})",slots.join(",")));
 }
 let order=if write.ids.is_empty(){order}else{"d.id ASC".into()};
 let rows=conn.query(&format!("SELECT count(*) {from}"),&params).await?;let total=int_at(&rows[0],0)? as usize;
 if !write.ids.is_empty()&&total!=write.ids.len(){return Err(AppError::NotFound);}
 if total>MAX_RECORDS{return Err(AppError::bad("Export supports up to 10,000 matching records. Narrow the view and export again."));}
 let related=resolve_related(conn,&principal,&collection,&write.related).await?;
 let columns=export_columns(&write.columns,&related);
 let mut lookups=RelatedCache::default();
 let mut bytes=vec![];
 if write.format=="csv"{csv_row(&mut bytes,&columns.iter().map(|c|c.header(&related)).collect::<Vec<_>>())?;}else{bytes.push(b'[');}
 let mut emitted=0;
 while emitted<total{
 let mut page_params=params.clone();page_params.push(SqlValue::Integer(100));let limit=format!("?{}",page_params.len());page_params.push(SqlValue::Integer(emitted as i64));let offset=format!("?{}",page_params.len());
 let rows=conn.query(&format!("SELECT d.id,d.body,d.created_at,d.updated_at {from} ORDER BY {order} LIMIT {limit} OFFSET {offset}"),&page_params).await?;
 if rows.is_empty(){return Err(anyhow::anyhow!("export snapshot changed").into());}
 for row in rows {
  let mut doc:DocumentView=store::document_from_row(&row)?;materialize(&collection.blueprint,&mut doc);
  let mut linked=Vec::with_capacity(related.len());
  for spec in &related{linked.push(lookups.records(conn,spec,&doc.body[&spec.field]).await?);}
  if write.format=="csv"{
   let cells=columns.iter().map(|column|match column{
    Column::Own(key)=>csv_cell(&doc.body[key]),
    Column::Related(index,key)=>{let spec=&related[*index];let values=linked[*index].iter().map(|(_,body)|body.as_ref().map(|b|b[key].clone()).unwrap_or(Value::Null));
     if spec.many{csv_cell(&Value::Array(values.collect()))}else{csv_cell(&values.into_iter().next().unwrap_or(Value::Null))}}
   }).collect::<Vec<_>>();csv_row(&mut bytes,&cells)?;
  }else{
   doc.body.as_object_mut().ok_or_else(||anyhow::anyhow!("invalid body"))?.retain(|key,_|write.columns.contains(key));doc.calculation_errors.retain(|key,_|write.columns.contains(key));
   let mut value=serde_json::to_value(&doc).map_err(anyhow::Error::from)?;
   if !related.is_empty(){
    let mut out=Map::new();
    for (spec,records) in related.iter().zip(&linked){
     let objects=records.iter().map(|(id,body)|{let mut object=Map::new();object.insert("id".into(),Value::String(id.clone()));for key in &spec.columns{object.insert(key.clone(),body.as_ref().map(|b|b[key].clone()).unwrap_or(Value::Null));}if body.is_none(){object.insert("missing".into(),Value::Bool(true));}Value::Object(object)}).collect::<Vec<_>>();
     out.insert(spec.field.clone(),if spec.many{Value::Array(objects)}else{objects.into_iter().next().unwrap_or(Value::Null)});
    }
    value["related"]=Value::Object(out);
   }
   if emitted>0{bytes.push(b',');}serde_json::to_writer(&mut bytes,&value).map_err(anyhow::Error::from)?;
  }
  emitted+=1;if bytes.len()>MAX_BYTES{return Err(AppError::bad("Export exceeds 25 MB. Choose fewer columns or narrow the view."));}
 }
 }
 if write.format=="json"{bytes.push(b']');}Ok((collection.name,bytes))
 }).await?;
    Ok((
        [
            (
                header::CONTENT_TYPE,
                if write.format == "csv" {
                    "text/csv; charset=utf-8".to_string()
                } else {
                    "application/json".to_string()
                },
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}.{}\"", write.format),
            ),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
        bytes,
    )
        .into_response())
}
fn csv_row(bytes: &mut Vec<u8>, cells: &[String]) -> Result<(), AppError> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(bytes);
    writer.write_record(cells).map_err(anyhow::Error::from)?;
    writer.flush().map_err(anyhow::Error::from)?;
    Ok(())
}
fn safe_csv_text(text: &str) -> String {
    if text
        .trim_start_matches(char::is_whitespace)
        .starts_with(['=', '+', '-', '@'])
        || text.starts_with(['\t', '\r', '\n'])
    {
        format!("'{text}")
    } else {
        text.into()
    }
}

/// Related fields allowed per export, and columns per related field.
const MAX_RELATED: usize = 20;
const MAX_RELATED_COLUMNS: usize = 50;

/// A validated `RelatedColumns`: the reference field, the collection it points
/// to, and whether it holds a list of references.
struct Related {
    field: String,
    many: bool,
    target: CollectionView,
    columns: Vec<String>,
}

async fn resolve_related(
    conn: &Conn,
    principal: &Principal,
    collection: &CollectionView,
    requested: &[RelatedColumns],
) -> Result<Vec<Related>, AppError> {
    if requested.len() > MAX_RELATED {
        return Err(AppError::bad(
            "Choose related columns from at most 20 fields",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut related = Vec::with_capacity(requested.len());
    for spec in requested {
        if !seen.insert(&spec.field) {
            return Err(AppError::bad(format!("'{}' is listed twice", spec.field)));
        }
        let field = collection
            .blueprint
            .fields
            .iter()
            .find(|f| f.name == spec.field)
            .ok_or_else(|| AppError::bad(format!("unknown field '{}'", spec.field)))?;
        let many = match (field.kind, field.items) {
            (FieldType::Reference, _) => false,
            (FieldType::Array, Some(ArrayItems::Reference)) => true,
            _ => {
                return Err(AppError::bad(format!(
                    "'{}' is not a reference field",
                    spec.field
                )));
            }
        };
        let name = field.collection.as_deref().unwrap_or_default();
        let target = if name == collection.name {
            collection.clone()
        } else {
            store::find_collection_by_name(conn, name)
                .await?
                .ok_or_else(|| AppError::bad(format!("unknown collection '{name}'")))?
        };
        // A key limited to one collection cannot read others through links.
        require_collection(principal, &target.id)?;
        let unique = spec
            .columns
            .iter()
            .collect::<std::collections::HashSet<_>>();
        if spec.columns.is_empty()
            || spec.columns.len() > MAX_RELATED_COLUMNS
            || unique.len() != spec.columns.len()
            || spec
                .columns
                .iter()
                .any(|key| !target.blueprint.fields.iter().any(|f| f.name == *key))
        {
            return Err(AppError::bad(format!(
                "Choose valid related columns for '{}'",
                spec.field
            )));
        }
        related.push(Related {
            field: spec.field.clone(),
            many,
            target,
            columns: spec.columns.clone(),
        });
    }
    Ok(related)
}

enum Column {
    Own(String),
    /// Index into the related list, and a column of the related collection.
    Related(usize, String),
}

impl Column {
    fn header(&self, related: &[Related]) -> String {
        match self {
            Column::Own(key) => key.clone(),
            Column::Related(index, key) => format!("{}.{key}", related[*index].field),
        }
    }
}

/// Own columns in order, each followed by any related columns of that field.
/// Related columns of fields not exported themselves come last.
fn export_columns(columns: &[String], related: &[Related]) -> Vec<Column> {
    fn expand(index: usize, spec: &Related) -> impl Iterator<Item = Column> + '_ {
        spec.columns
            .iter()
            .map(move |key| Column::Related(index, key.clone()))
    }
    let mut out = Vec::new();
    for key in columns {
        out.push(Column::Own(key.clone()));
        for (index, spec) in related.iter().enumerate().filter(|(_, s)| s.field == *key) {
            out.extend(expand(index, spec));
        }
    }
    for (index, spec) in related.iter().enumerate() {
        if !columns.contains(&spec.field) {
            out.extend(expand(index, spec));
        }
    }
    out
}

/// Referenced records, looked up once per export and calculated with their
/// own collection's formulas. `None` marks a reference to a missing record.
#[derive(Default)]
struct RelatedCache {
    bodies: HashMap<(String, String), Option<Value>>,
}

impl RelatedCache {
    async fn records(
        &mut self,
        conn: &Conn,
        spec: &Related,
        value: &Value,
    ) -> anyhow::Result<Vec<(String, Option<Value>)>> {
        let ids: Vec<String> = match value {
            Value::String(id) => vec![id.clone()],
            Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            _ => vec![],
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let key = (spec.target.id.clone(), id.clone());
            if !self.bodies.contains_key(&key) {
                let body =
                    store::find_document(conn, &spec.target.id, &id)
                        .await?
                        .map(|mut doc| {
                            materialize(&spec.target.blueprint, &mut doc);
                            doc.body
                        });
                self.bodies.insert(key.clone(), body);
            }
            out.push((id, self.bodies[&key].clone()));
        }
        Ok(out)
    }
}

fn csv_cell(value: &Value) -> String {
    if value.is_null() {
        String::new()
    } else if let Some(text) = value.as_str() {
        safe_csv_text(text)
    } else {
        value.to_string()
    }
}
