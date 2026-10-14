use crate::{
    auth::Editor,
    content::{check_body, workspace_collection},
    error::AppError,
    state::AppState,
    store,
};
use axum::{
    Json,
    extract::{Multipart, Path, State},
};
use calamine::{Data, Reader, open_workbook_auto_from_rs};
use serde_json::{Map, Value};
use std::{collections::HashSet, io::Cursor};
use vulse_model::{Blueprint, FieldType, ImportIssue, ImportResult};

pub const MAX_BYTES: usize = 50 * 1024 * 1024;
const MAX_ROWS: usize = 50_000;
/// Uncompressed size limit for an XLSX workbook and for each worksheet.
const MAX_EXPANDED: u64 = 200 * 1024 * 1024;
/// Row problems reported per import; the rest are counted.
const MAX_ISSUES: usize = 500;
const MAX_COLUMNS: usize = 200;
#[derive(Debug)]
struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    sheets: Vec<String>,
    sheet: String,
    first_row: usize,
}

pub async fn import_file(
    State(state): State<AppState>,
    Editor(principal): Editor,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<ImportResult>, AppError> {
    let conn = state.db.connect().await?;
    workspace_collection(&conn, &principal, &id).await?;
    let mut file = None;
    let mut mapping = None::<Vec<Option<String>>>;
    let mut sheet = String::new();
    let mut commit = false;
    let mut mode = String::from("append");
    let mut match_field = String::new();
    let mut has_headers = true;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::bad(e.to_string()))?
    {
        match field.name().unwrap_or("") {
            "file" => {
                if file.is_some() {
                    return Err(AppError::bad("send one file"));
                }
                let name = field.file_name().unwrap_or("").to_owned();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::bad(e.to_string()))?;
                if bytes.len() > MAX_BYTES {
                    return Err(AppError::PayloadTooLarge);
                }
                file = Some((name, bytes.to_vec()));
            }
            "mapping" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad(e.to_string()))?;
                mapping = Some(
                    serde_json::from_str(&text).map_err(|_| AppError::bad("invalid mapping"))?,
                );
            }
            "sheet" => {
                sheet = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad(e.to_string()))?
            }
            "mode" => {
                mode = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad(e.to_string()))?
            }
            "match_field" => {
                match_field = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad(e.to_string()))?
            }
            "commit" => {
                let value = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad(e.to_string()))?;
                if value != "true" && value != "false" {
                    return Err(AppError::bad("commit must be true or false"));
                }
                commit = value == "true";
            }
            "has_headers" => {
                let value = field
                    .text()
                    .await
                    .map_err(|e| AppError::bad(e.to_string()))?;
                if value != "true" && value != "false" {
                    return Err(AppError::bad("has_headers must be true or false"));
                }
                has_headers = value == "true";
            }
            _ => return Err(AppError::bad("unknown import option")),
        }
    }
    if !["append", "upsert"].contains(&mode.as_str()) {
        return Err(AppError::bad("mode must be append or upsert"));
    }
    let (name, bytes) = file.ok_or_else(|| AppError::bad("missing file"))?;
    let table = tokio::task::spawn_blocking(move || parse_file(&name, &bytes, &sheet, has_headers))
        .await
        .map_err(|e| anyhow::anyhow!(e))?
        .map_err(AppError::bad)?;
    // Re-read and validate inside the write transaction so schema changes cannot bypass validation.
    let result = conn
        .with_app_transaction(commit, async |conn| {
            let collection = workspace_collection(conn, &principal, &id).await?;
            let lookups = crate::lookups::load(conn, &collection.blueprint).await?;
            let mapping = match mapping {
                Some(m) => m,
                None => table
                    .headers
                    .iter()
                    .map(|h| {
                        collection
                            .blueprint
                            .fields
                            .iter()
                            .find(|f| {
                                f.name == h.trim().to_ascii_lowercase()
                                    && f.kind != FieldType::Calculated
                            })
                            .map(|f| f.name.clone())
                    })
                    .collect(),
            };
            validate_mapping(&collection.blueprint, &table.headers, &mapping)?;
            let mut result = ImportResult {
                headers: table.headers.clone(),
                sheets: table.sheets.clone(),
                sheet: table.sheet.clone(),
                mapping: mapping.clone(),
                row_count: 0,
                preview: vec![],
                errors: vec![],
                imported: 0,created:0,updated:0,unchanged:0,preview_actions:vec![],
            };
            if mapping.iter().all(Option::is_none) {
                result.errors.push(ImportIssue {
                    row: 1,
                    field: None,
                    message: "Map at least one column to a field".into(),
                });
            }
            let mut existing=std::collections::HashMap::new();
            if mode=="upsert" {
                if !collection.blueprint.fields.iter().any(|f|f.name==match_field&&matches!(f.kind,FieldType::String|FieldType::Number)){return Err(AppError::bad("Choose a string or number field for matching records"));}
                if !mapping.iter().any(|m|m.as_deref()==Some(match_field.as_str())){return Err(AppError::bad("The matching field must be mapped to an imported column"));}
                for(doc,body)in store::list_document_bodies(conn,&id).await? {
                    if let Some(key)=match_key(&body[&match_field]){if existing.insert(key,(doc,body)).is_some(){result.errors.push(ImportIssue{row:0,field:Some(match_field.clone()),message:"Existing records contain duplicate matching values. Resolve them before importing updates.".into()});}}
                }
            }
            let mut seen_keys=HashSet::new();
            let mut bodies = vec![];
            for (i, row) in table.rows.iter().enumerate() {
                if row.iter().all(|c| c.trim().is_empty()) {
                    continue;
                }
                result.row_count += 1;
                let row_number = table.first_row + i;
                let mut obj = Map::new();
                let mut failed = false;
                for (column, target) in mapping.iter().enumerate() {
                    let Some(target) = target else {
                        continue;
                    };
                    let f = collection
                        .blueprint
                        .fields
                        .iter()
                        .find(|f| &f.name == target)
                        .unwrap();
                    let cell = row.get(column).map(String::as_str).unwrap_or("");
                    if cell.trim().is_empty() {
                        continue;
                    }
                    match convert(f.kind, &decimal_comma(f.kind, cell)) {
                        Ok(v) => {
                            obj.insert(target.clone(), v);
                        }
                        Err(message) => {
                            failed = true;
                            result.errors.push(ImportIssue {
                                row: row_number,
                                field: Some(target.clone()),
                                message,
                            });
                        }
                    }
                }
                let mut body=Value::Object(obj);
                let mut target=None;let mut same=false;
                if mode=="upsert"{
                    match match_key(&body[&match_field]) {
                        None=>{failed=true;result.errors.push(ImportIssue{row:row_number,field:Some(match_field.clone()),message:"Every imported row needs a nonempty matching value".into()});},
                        Some(key)=>{
                            if !seen_keys.insert(key.clone()){failed=true;result.errors.push(ImportIssue{row:row_number,field:Some(match_field.clone()),message:"Duplicate matching value in this file".into()});}
                            if let Some((doc,previous))=existing.get(&key){let mut merged=previous.as_object().cloned().unwrap_or_default();merged.extend(body.as_object().unwrap().clone());body=Value::Object(merged);same=semantic_eq(previous,&body);target=Some(doc.clone());}
                        }
                    }
                }
                let action=if target.is_none(){"create"}else if same{"unchanged"}else{"update"};
                if !failed {
                    match check_body(
                        conn,
                        &id,
                        &collection.name,
                        &collection.blueprint,
                        &body,
                    )
                    .await
                    {
                        Ok(()) => {}
                        Err(AppError::BadRequest(message)) => {
                            failed = true;
                            result.errors.push(ImportIssue {
                                row: row_number,
                                field: None,
                                message,
                            });
                        }
                        Err(e) => return Err(e),
                    }
                    let (computed, errors) =
                        vulse_model::calculate_document_with(&collection.blueprint, &body, &lookups);
                    for (field, message) in errors {
                        failed = true;
                        result.errors.push(ImportIssue {
                            row: row_number,
                            field: Some(field),
                            message,
                        });
                    }
                    if result.preview.len() < 10 {
                        result.preview.push(computed);
                        result.preview_actions.push(action.into());
                    }
                }
                if !failed {
                    match action{"create"=>result.created+=1,"update"=>result.updated+=1,_=>result.unchanged+=1}
                    bodies.push((target,body,action));
                }
            }
            if result.row_count == 0 {
                result.errors.push(ImportIssue {
                    row: table.first_row,
                    field: None,
                    message: "No data rows found".into(),
                });
            }
            if result.errors.len() > MAX_ISSUES {
                let more = result.errors.len() - MAX_ISSUES;
                result.errors.truncate(MAX_ISSUES);
                result.errors.push(ImportIssue {
                    row: 0,
                    field: None,
                    message: format!("…and {more} more problems. Fix the ones above and preview again."),
                });
            }
            if commit && result.errors.is_empty() {
                let now = crate::timeutil::now_rfc3339();
                for (target,body,action) in bodies {
                    if action=="unchanged"{continue;}
                    if let Some(doc)=target{store::update_document(conn,&id,&doc,&body,&now).await?;}else{store::insert_document(conn,&crate::auth::new_id(),&id,&body,&now).await?;}
                    result.imported += 1;
                }
            }
            Ok(result)
        })
        .await?;
    Ok(Json(result))
}
// Exact decimal normalization avoids treating large distinct integers as equal.
fn match_key(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return if text.trim().is_empty() {
            None
        } else {
            Some(format!("s:{text}"))
        };
    }
    if !value.is_number() {
        return None;
    }
    let raw = value.to_string();
    let (negative, raw) = if let Some(raw) = raw.strip_prefix('-') {
        (true, raw)
    } else {
        (false, raw.as_str())
    };
    let mut parts = raw.split(['e', 'E']);
    let mantissa = parts.next()?;
    let exponent = parts
        .next()
        .map(|e| e.parse::<i32>().ok())
        .unwrap_or(Some(0))?;
    let decimals = mantissa.split('.').nth(1).map(str::len).unwrap_or(0) as i32;
    let mut digits = mantissa
        .replace('.', "")
        .trim_start_matches('0')
        .to_string();
    let mut exponent = exponent - decimals;
    if digits.is_empty() {
        return Some("n:0".into());
    }
    while digits.ends_with('0') {
        digits.pop();
        exponent += 1;
    }
    Some(format!(
        "n:{}{}e{}",
        if negative { "-" } else { "" },
        digits,
        exponent
    ))
}
fn semantic_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => match_key(a) == match_key(b),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| semantic_eq(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|other| semantic_eq(v, other)))
        }
        _ => a == b,
    }
}
fn validate_mapping(
    bp: &Blueprint,
    headers: &[String],
    mapping: &[Option<String>],
) -> Result<(), AppError> {
    if mapping.len() != headers.len() {
        return Err(AppError::bad("mapping must have one target per column"));
    }
    let mut seen = HashSet::new();
    for n in mapping.iter().flatten() {
        if !seen.insert(n) {
            return Err(AppError::bad(format!(
                "field '{n}' is mapped more than once"
            )));
        }
        if !bp
            .fields
            .iter()
            .any(|f| &f.name == n && f.kind != FieldType::Calculated)
        {
            return Err(AppError::bad(format!("unknown or calculated target '{n}'")));
        }
    }
    Ok(())
}
/// Decimal-comma locales write 33,47 in files; a lone comma without a point
/// in a number cell is read as the decimal mark.
fn decimal_comma(kind: FieldType, cell: &str) -> std::borrow::Cow<'_, str> {
    if kind == FieldType::Number && cell.matches(',').count() == 1 && !cell.contains('.') {
        cell.replace(',', ".").into()
    } else {
        cell.into()
    }
}
pub(crate) fn convert(kind: FieldType, cell: &str) -> Result<Value, String> {
    match kind {
        FieldType::String | FieldType::Asset | FieldType::Reference => {
            Ok(Value::String(cell.into()))
        }
        FieldType::Number => {
            let n: Value =
                serde_json::from_str(cell.trim()).map_err(|_| "Expected a finite number")?;
            if !n.is_number() {
                return Err("Expected a number".into());
            }
            Ok(n)
        }
        FieldType::Date => import_date(cell.trim()).map(Value::String),
        FieldType::Boolean => match cell.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(Value::Bool(true)),
            "false" | "0" | "no" => Ok(Value::Bool(false)),
            _ => Err("Expected true/false, yes/no or 1/0".into()),
        },
        FieldType::Json | FieldType::Array | FieldType::PortableText => {
            serde_json::from_str(cell).map_err(|_| "Expected valid JSON".into())
        }
        FieldType::Calculated => Err("Calculated fields are read-only".into()),
    }
}
/// Accepts `YYYY-MM-DD`, or a date and time at exactly midnight as written
/// for spreadsheet date cells. Other times are refused rather than dropped.
fn import_date(cell: &str) -> Result<String, String> {
    let error = || "Expected a date as YYYY-MM-DD".to_string();
    let date = cell.get(..10).ok_or_else(error)?;
    if !vulse_model::is_date(date) {
        return Err(error());
    }
    let time = cell[10..].trim_start_matches(['T', ' ']);
    if time.is_empty() || time.trim_end_matches(['0', ':', '.']).is_empty() {
        Ok(date.to_string())
    } else {
        Err("Expected a date without a time of day".into())
    }
}
/// Date-formatted spreadsheet cells become ISO text, date-only at midnight.
fn excel_datetime(dt: &calamine::ExcelDateTime) -> String {
    let (y, mo, d, h, mi, s, ms) = dt.to_ymd_hms_milli();
    if (h, mi, s, ms) == (0, 0, 0, 0) {
        format!("{y:04}-{mo:02}-{d:02}")
    } else {
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}")
    }
}
/// Picks `,`, `;` or tab by counting each outside quotes on the header line,
/// since spreadsheets in decimal-comma locales export semicolon-separated CSV.
fn csv_delimiter(text: &str) -> u8 {
    let mut counts = [(b',', 0), (b';', 0), (b'\t', 0)];
    let mut quoted = false;
    for byte in text.bytes() {
        match byte {
            b'"' => quoted = !quoted,
            b'\n' | b'\r' if !quoted => break,
            _ if !quoted => {
                if let Some(entry) = counts.iter_mut().find(|(d, _)| *d == byte) {
                    entry.1 += 1;
                }
            }
            _ => {}
        }
    }
    counts
        .iter()
        .max_by_key(|(_, n)| *n)
        .filter(|(_, n)| *n > 0)
        .map_or(b',', |(d, _)| *d)
}
fn parse_file(name: &str, bytes: &[u8], sheet: &str, has_headers: bool) -> Result<Table, String> {
    if bytes.len() > MAX_BYTES {
        return Err("File exceeds 50 MB".into());
    }
    if name.to_ascii_lowercase().ends_with(".csv") {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        let text = std::str::from_utf8(bytes).map_err(|_| "CSV must use UTF-8 encoding")?;
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(csv_delimiter(text))
            .has_headers(false)
            .from_reader(bytes);
        let mut records = reader.records();
        let headers = if has_headers {
            let first = records.next().ok_or("CSV file is empty")?;
            first
                .map_err(|e| e.to_string())?
                .iter()
                .map(str::to_owned)
                .collect()
        } else {
            vec![]
        };
        let mut rows = vec![];
        for row in records {
            let row = row.map_err(|e| e.to_string())?;
            if rows.len() >= MAX_ROWS {
                return Err("Maximum 50,000 data rows per import".into());
            }
            rows.push(row.iter().map(str::to_owned).collect::<Vec<_>>());
        }
        let headers = if has_headers {
            headers
        } else {
            numbered_headers(rows.first().map_or(0, Vec::len))
        };
        validate_headers(&headers)?;
        return Ok(Table {
            headers,
            rows,
            sheets: vec![],
            sheet: String::new(),
            first_row: if has_headers { 2 } else { 1 },
        });
    }
    if !name.to_ascii_lowercase().ends_with(".xlsx") {
        return Err(
            "Choose a .csv or .xlsx file; save legacy .xls workbooks as .xlsx first".into(),
        );
    }
    if name.to_ascii_lowercase().ends_with(".xlsx") {
        preflight_xlsx(bytes)?;
    }
    let mut book = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|e| format!("Cannot read workbook: {e}"))?;
    let sheets = book.sheet_names().to_vec();
    let selected = if sheet.is_empty() {
        sheets.first().ok_or("Workbook has no worksheets")?.clone()
    } else {
        sheet.into()
    };
    if !sheets.contains(&selected) {
        return Err("Unknown worksheet".into());
    }
    let range = book.worksheet_range(&selected).map_err(|e| e.to_string())?;
    let header_rows = usize::from(has_headers);
    if range.height() > MAX_ROWS + header_rows || range.width() > MAX_COLUMNS {
        return Err("Maximum 50,000 rows and 200 columns per worksheet".into());
    }
    let first_row = range.start().map_or(1, |(row, _)| row as usize + 1) + header_rows;
    let mut rows = range.rows();
    let headers = if has_headers {
        rows.next()
            .ok_or("Worksheet is empty")?
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    } else {
        numbered_headers(range.width())
    };
    validate_headers(&headers)?;
    let rows = rows
        .enumerate()
        .map(|(i, row)| {
            row.iter()
                .map(|c| match c {
                    Data::Error(e) => {
                        Err(format!("Row {}: spreadsheet error {e:?}", first_row + i))
                    }
                    Data::DateTimeIso(s) => Ok(s.clone()),
                    Data::DateTime(dt) if dt.is_datetime() => Ok(excel_datetime(dt)),
                    _ => Ok(c.to_string()),
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Table {
        headers,
        rows,
        sheets,
        sheet: selected,
        first_row,
    })
}
/// Names columns of a file without a header row.
fn numbered_headers(count: usize) -> Vec<String> {
    (1..=count).map(|i| format!("Column {i}")).collect()
}
fn validate_headers(headers: &[String]) -> Result<(), String> {
    if headers.is_empty() || headers.len() > MAX_COLUMNS {
        return Err("Expected 1–200 columns".into());
    }
    let mut names = HashSet::new();
    for h in headers {
        let n = h.trim().to_ascii_lowercase();
        if n.is_empty() || !names.insert(n) {
            return Err("Column headers must be nonempty and unique".into());
        }
    }
    Ok(())
}

// Check archive expansion and sparse coordinates before Calamine allocates a dense worksheet.
fn preflight_xlsx(bytes: &[u8]) -> Result<(), String> {
    use quick_xml::events::Event;
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("Invalid XLSX archive: {e}"))?;
    if archive.len() > 2000 {
        return Err("Workbook contains too many archive entries".into());
    }
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        total = total
            .checked_add(entry.size())
            .ok_or("Workbook is too large")?;
        if total > MAX_EXPANDED {
            return Err("Expanded workbook exceeds 200 MB".into());
        }
        if !entry.name().starts_with("xl/worksheets/") || !entry.name().ends_with(".xml") {
            continue;
        }
        let mut xml = Vec::new();
        entry
            .by_ref()
            .take(MAX_EXPANDED + 1)
            .read_to_end(&mut xml)
            .map_err(|e| e.to_string())?;
        if xml.len() as u64 > MAX_EXPANDED {
            return Err("Expanded worksheet exceeds 200 MB".into());
        }
        let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
        loop {
            match reader.read_event().map_err(|e| e.to_string())? {
                Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"c" => {
                    for attr in e.attributes() {
                        let attr = attr.map_err(|e| e.to_string())?;
                        if attr.key.as_ref() == b"r" {
                            let value = std::str::from_utf8(&attr.value)
                                .map_err(|_| "Invalid cell coordinate")?;
                            check_coordinate(value)?;
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }
    Ok(())
}
fn check_coordinate(value: &str) -> Result<(), String> {
    let mut column = 0usize;
    let mut row = 0usize;
    let mut digits = false;
    for c in value.chars() {
        if c.is_ascii_uppercase() && !digits {
            column = column
                .saturating_mul(26)
                .saturating_add(c as usize - 'A' as usize + 1);
        } else if c.is_ascii_digit() {
            digits = true;
            row = row
                .saturating_mul(10)
                .saturating_add(c as usize - '0' as usize);
        } else {
            return Err("Invalid cell coordinate".into());
        }
    }
    if row == 0 || column == 0 || row > MAX_ROWS + 1 || column > MAX_COLUMNS {
        return Err("Workbook cells must be within the first 1,001 rows and 200 columns; export a smaller worksheet".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn workbook(cell: &str) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let entries=[
            ("[Content_Types].xml",r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string()),
            ("_rels/.rels",r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into()),
            ("xl/workbook.xml",r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="First" sheetId="1" r:id="rId1"/><sheet name="Second" sheetId="2" r:id="rId2"/></sheets></workbook>"#.into()),
            ("xl/_rels/workbook.xml.rels",r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/></Relationships>"#.into()),
            ("xl/worksheets/sheet1.xml",format!(r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>price</t></is></c></row><row r="2">{cell}</row></sheetData></worksheet>"#)),
            ("xl/worksheets/sheet2.xml",r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>title</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Hello</t></is></c></row></sheetData></worksheet>"#.into()),
        ];
        for (name, data) in entries {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    #[test]
    fn parses_xlsx_worksheets_and_cached_formula_values() {
        let bytes = workbook(r#"<c r="A2"><f>10+2</f><v>12</v></c>"#);
        let first = parse_file("data.xlsx", &bytes, "", true).unwrap();
        assert_eq!(first.sheets, vec!["First", "Second"]);
        assert_eq!(first.rows[0][0], "12");
        let second = parse_file("data.xlsx", &bytes, "Second", true).unwrap();
        assert_eq!(second.headers, vec!["title"]);
        assert_eq!(second.rows[0][0], "Hello");
        assert!(parse_file("data.xlsx", &bytes, "Missing", true).is_err());
        let error = workbook(r#"<c r="A2" t="e"><v>#DIV/0!</v></c>"#);
        assert!(
            parse_file("data.xlsx", &error, "", true)
                .unwrap_err()
                .contains("Row 2")
        );
    }
    /// One sheet whose second row holds a date cell (built-in format 14) and
    /// a date-time cell (format 22) for the same serial day.
    fn dated_workbook() -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let entries = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Dates" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
            ),
            (
                "xl/styles.xml",
                r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/><xf numFmtId="22" applyNumberFormat="1"/></cellXfs></styleSheet>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>due</t></is></c><c r="B1" t="inlineStr"><is><t>at</t></is></c></row><row r="2"><c r="A2" s="1"><v>45943</v></c><c r="B2" s="2"><v>45943.5</v></c></row></sheetData></worksheet>"#,
            ),
        ];
        for (name, data) in entries {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    #[test]
    fn numbers_accept_a_decimal_comma() {
        let number = |cell| convert(FieldType::Number, &decimal_comma(FieldType::Number, cell));
        assert_eq!(number("33,47").unwrap(), serde_json::json!(33.47));
        assert_eq!(number(" -0,5 ").unwrap(), serde_json::json!(-0.5));
        assert_eq!(number("12.5").unwrap(), serde_json::json!(12.5));
        for bad in ["1,234.5", "1,2,3", ",5", "abc"] {
            assert!(number(bad).is_err(), "{bad}");
        }
        assert_eq!(decimal_comma(FieldType::String, "3,5"), "3,5");
    }
    #[test]
    fn spreadsheet_and_text_dates_convert_to_iso_dates() {
        let table = parse_file("dates.xlsx", &dated_workbook(), "", true).unwrap();
        assert_eq!(table.rows[0], vec!["2025-10-13", "2025-10-13T12:00:00"]);
        assert_eq!(
            convert(FieldType::Date, &table.rows[0][0]).unwrap(),
            "2025-10-13"
        );
        assert!(convert(FieldType::Date, &table.rows[0][1]).is_err());
        assert_eq!(
            convert(FieldType::Date, " 2024-02-29T00:00:00 ").unwrap(),
            "2024-02-29"
        );
        for bad in [
            "2023-02-29",
            "13/10/2025",
            "2025-1-5",
            "45943",
            "2025-10-13Z",
            "",
        ] {
            assert!(convert(FieldType::Date, bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn rejects_sparse_workbooks_and_invalid_csv() {
        let bytes = workbook(r#"<c r="XFD1048576"><v>12</v></c>"#);
        assert!(parse_file("data.xlsx", &bytes, "", true).is_err());
        for csv in ["a,a\n1,2\n", "a,b\n1\n", ",b\n1,2\n"] {
            assert!(parse_file("data.csv", csv.as_bytes(), "", true).is_err());
        }
        assert!(parse_file("data.xlsx", b"not a workbook", "", true).is_err());
        let csv = parse_file(
            "data.csv",
            b"\xef\xbb\xbftitle,price\n\"a,b\",12\n",
            "",
            true,
        )
        .unwrap();
        assert_eq!(csv.headers[0], "title");
        assert_eq!(csv.rows[0][0], "a,b");
        let csv = "year;name;share;kind;a;b\n2023;Byfjordparken;27,55;Ordinaer;;\n2023;Småbergan;8;Ordinaer;;\n";
        let csv = parse_file("data.csv", csv.as_bytes(), "", true).unwrap();
        assert_eq!(csv.headers.len(), 6);
        assert_eq!(csv.rows[0][2], "27,55");
        assert_eq!(csv.rows[1][2], "8");
        assert_eq!(csv_delimiter("\"a;b\",c\n1,2"), b',');
        let csv = "2023;920143067;Åmot montessoribarnehage SA;Åmot kommune;Privat;3422;3,55;Ordinaer;;;\n2023;989009427;Småbergan barnehage ;Trondheim kommune;Privat;5001;8;Ordinaer;;;\n";
        let csv = parse_file("data.csv", csv.as_bytes(), "", false).unwrap();
        assert_eq!(csv.headers.len(), 11);
        assert_eq!(csv.headers[0], "Column 1");
        assert_eq!(csv.rows.len(), 2);
        assert_eq!(csv.rows[0][6], "3,55");
        assert_eq!(csv.first_row, 1);
        assert_eq!(csv_delimiter("a\tb\n"), b'\t');
        let most = "a\n".to_owned() + &"1\n".repeat(MAX_ROWS);
        assert_eq!(
            parse_file("data.csv", most.as_bytes(), "", true)
                .unwrap()
                .rows
                .len(),
            MAX_ROWS
        );
        let huge = "a\n".to_owned() + &"1\n".repeat(MAX_ROWS + 1);
        assert!(parse_file("data.csv", huge.as_bytes(), "", true).is_err());
    }
}
