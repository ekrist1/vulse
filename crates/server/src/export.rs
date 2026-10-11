use crate::{
    auth::AuthUser,
    content::{materialize, owned_collection},
    db::{SqlValue, int_at},
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
use vulse_model::{DocumentView, ExportRequest};
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
 let collection=owned_collection(conn,&principal,&id).await?;
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
 let mut bytes=vec![];
 if write.format=="csv"{csv_row(&mut bytes,&write.columns)?;}else{bytes.push(b'[');}
 let mut emitted=0;
 while emitted<total{
 let mut page_params=params.clone();page_params.push(SqlValue::Integer(100));let limit=format!("?{}",page_params.len());page_params.push(SqlValue::Integer(emitted as i64));let offset=format!("?{}",page_params.len());
 let rows=conn.query(&format!("SELECT d.id,d.body,d.created_at,d.updated_at {from} ORDER BY {order} LIMIT {limit} OFFSET {offset}"),&page_params).await?;
 if rows.is_empty(){return Err(anyhow::anyhow!("export snapshot changed").into());}
 for row in rows {
  let mut doc:DocumentView=store::document_from_row(&row)?;materialize(&collection.blueprint,&mut doc);
  if write.format=="csv"{
   let cells=write.columns.iter().map(|key|{let value=&doc.body[key];if value.is_null(){String::new()}else if let Some(s)=value.as_str(){safe_csv_text(s)}else{value.to_string()}}).collect::<Vec<_>>();csv_row(&mut bytes,&cells)?;
  }else{
   doc.body.as_object_mut().ok_or_else(||anyhow::anyhow!("invalid body"))?.retain(|key,_|write.columns.contains(key));doc.calculation_errors.retain(|key,_|write.columns.contains(key));
   if emitted>0{bytes.push(b',');}serde_json::to_writer(&mut bytes,&doc).map_err(anyhow::Error::from)?;
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
