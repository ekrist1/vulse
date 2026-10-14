//! A typed table per collection for searching, filtering, sorting and
//! summarizing records.
//!
//! Record bodies in `documents` stay the source of truth. Each collection
//! also has a table with one row per record and typed columns per field,
//! kept up to date on every write and rebuilt from the bodies when the
//! collection's fields change. Its indexes let a sorted or filtered page
//! read only the rows it shows.
//!
//! Every field has three columns, all `NULL` when the record has no value:
//! `_t` holds the lowercase display text, `_n` the number for numeric
//! values and `_r` the stored JSON. Calculated fields add `_s`, a sort key
//! that is the number when there is one and the text otherwise.

use serde_json::Value;
use vulse_model::{Blueprint, FieldDef, FieldType, Lookups};

use crate::db::{Conn, SqlValue, text_at};

/// Parameters per `INSERT` while rebuilding a table.
const BATCH_PARAMS: usize = 20_000;

/// The quoted table name for a collection.
pub(crate) fn table(collection_id: &str) -> String {
    let name: String = collection_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("\"rows_{name}\"")
}

/// A quoted column of a field: `t`, `n`, `r` or `s`.
pub(crate) fn column(field: &str, part: char) -> String {
    format!("\"f_{field}_{part}\"")
}

/// The column a field sorts by.
pub(crate) fn sort_column(field: &FieldDef) -> String {
    match field.kind {
        FieldType::Number => column(&field.name, 'n'),
        FieldType::Calculated => column(&field.name, 's'),
        _ => column(&field.name, 't'),
    }
}

/// Fields worth a sort index; large or structured values are not sorted by.
fn indexed(field: &FieldDef) -> bool {
    !matches!(
        field.kind,
        FieldType::Json | FieldType::PortableText | FieldType::Array
    )
}

fn columns(blueprint: &Blueprint) -> Vec<String> {
    let mut out = vec![
        "id".to_string(),
        "created_at".into(),
        "updated_at".into(),
        "search".into(),
    ];
    for field in &blueprint.fields {
        for part in ['t', 'n', 'r'] {
            out.push(column(&field.name, part));
        }
        if field.kind == FieldType::Calculated {
            out.push(column(&field.name, 's'));
        }
    }
    out
}

/// Column values for one record, after `columns`' first three.
fn values(blueprint: &Blueprint, lookups: &Lookups, body: &Value) -> Vec<SqlValue> {
    let (body, _) = vulse_model::calculate_document_with(blueprint, body, lookups);
    let mut search = Vec::new();
    let mut out = vec![SqlValue::Null];
    for field in &blueprint.fields {
        let value = &body[&field.name];
        if value.is_null() {
            out.extend([SqlValue::Null, SqlValue::Null, SqlValue::Null]);
            if field.kind == FieldType::Calculated {
                out.push(SqlValue::Null);
            }
            continue;
        }
        let text = vulse_model::display_value(value).to_lowercase();
        let number = value.as_f64().map_or(SqlValue::Null, SqlValue::Real);
        if !text.is_empty() {
            search.push(text.clone());
        }
        out.push(SqlValue::Text(text.clone()));
        out.push(number.clone());
        out.push(SqlValue::Text(value.to_string()));
        if field.kind == FieldType::Calculated {
            out.push(match number {
                SqlValue::Null => SqlValue::Text(text),
                number => number,
            });
        }
    }
    // Values are kept apart so a search cannot match across two fields.
    out[0] = SqlValue::Text(search.join("\n"));
    out
}

/// Whether a blueprint change affects the table's columns or values.
pub(crate) fn shape_changed(old: &Blueprint, new: &Blueprint) -> bool {
    let shape = |b: &Blueprint| {
        b.fields
            .iter()
            .map(|f| {
                (
                    f.name.clone(),
                    f.kind,
                    f.items,
                    f.formula.clone(),
                    f.collection.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    shape(old) != shape(new)
}

/// Creates the collection's empty table, replacing any previous one.
pub(crate) async fn create(
    conn: &Conn,
    collection_id: &str,
    blueprint: &Blueprint,
) -> anyhow::Result<()> {
    create_table(conn, collection_id, blueprint).await?;
    create_indexes(conn, collection_id, blueprint).await
}

async fn create_table(
    conn: &Conn,
    collection_id: &str,
    blueprint: &Blueprint,
) -> anyhow::Result<()> {
    let table = table(collection_id);
    let mut definitions = vec![
        "id TEXT PRIMARY KEY".to_string(),
        "created_at TEXT NOT NULL".into(),
        "updated_at TEXT NOT NULL".into(),
        "search TEXT NOT NULL".into(),
    ];
    for field in &blueprint.fields {
        definitions.push(format!("{} TEXT", column(&field.name, 't')));
        definitions.push(format!("{} REAL", column(&field.name, 'n')));
        definitions.push(format!("{} TEXT", column(&field.name, 'r')));
        if field.kind == FieldType::Calculated {
            // No declared type, so text and numbers are both stored as given.
            definitions.push(column(&field.name, 's'));
        }
    }
    conn.execute(&format!("DROP TABLE IF EXISTS {table}"), &[])
        .await?;
    conn.execute(
        &format!("CREATE TABLE {table} ({})", definitions.join(", ")),
        &[],
    )
    .await?;
    Ok(())
}

async fn create_indexes(
    conn: &Conn,
    collection_id: &str,
    blueprint: &Blueprint,
) -> anyhow::Result<()> {
    let table = table(collection_id);
    let base = table.trim_matches('"');
    conn.execute(
        &format!("CREATE INDEX \"{base}_created\" ON {table}(created_at, id)"),
        &[],
    )
    .await?;
    conn.execute(
        &format!("CREATE INDEX \"{base}_updated\" ON {table}(updated_at, id)"),
        &[],
    )
    .await?;
    for (index, field) in blueprint.fields.iter().filter(|f| indexed(f)).enumerate() {
        conn.execute(
            &format!(
                "CREATE INDEX \"{base}_f{index}\" ON {table}({}, id)",
                sort_column(field)
            ),
            &[],
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn drop(conn: &Conn, collection_id: &str) -> anyhow::Result<()> {
    conn.execute(
        &format!("DROP TABLE IF EXISTS {}", table(collection_id)),
        &[],
    )
    .await?;
    Ok(())
}

/// Writes one record's row from its stored body.
pub(crate) async fn sync(
    conn: &Conn,
    collection_id: &str,
    blueprint: &Blueprint,
    lookups: &Lookups,
    id: &str,
    body: &Value,
) -> anyhow::Result<()> {
    let names = columns(blueprint);
    let mut params = vec![SqlValue::Text(id.into())];
    params.extend(values(blueprint, lookups, body));
    let slots: Vec<String> = (2..=params.len()).map(|i| format!("?{i}")).collect();
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO {} ({}) SELECT id, created_at, updated_at, {} FROM documents WHERE id=?1",
            table(collection_id),
            names.join(", "),
            slots.join(", ")
        ),
        &params,
    )
    .await?;
    Ok(())
}

pub(crate) async fn remove(conn: &Conn, collection_id: &str, id: &str) -> anyhow::Result<()> {
    conn.execute(
        &format!("DELETE FROM {} WHERE id=?1", table(collection_id)),
        &[SqlValue::Text(id.into())],
    )
    .await?;
    Ok(())
}

/// Recreates the table and fills it from every record body.
pub(crate) async fn rebuild(
    conn: &Conn,
    collection_id: &str,
    blueprint: &Blueprint,
    lookups: &Lookups,
) -> anyhow::Result<()> {
    create_table(conn, collection_id, blueprint).await?;
    let names = columns(blueprint);
    let rows = conn
        .query(
            "SELECT id, body, created_at, updated_at FROM documents WHERE collection_id=?1",
            &[SqlValue::Text(collection_id.into())],
        )
        .await?;
    for chunk in rows.chunks((BATCH_PARAMS / names.len()).max(1)) {
        let mut params = Vec::new();
        let mut tuples = Vec::new();
        for row in chunk {
            let body: Value = serde_json::from_str(&text_at(row, 1)?)?;
            let start = params.len();
            params.push(SqlValue::Text(text_at(row, 0)?));
            params.push(SqlValue::Text(text_at(row, 2)?));
            params.push(SqlValue::Text(text_at(row, 3)?));
            params.extend(values(blueprint, lookups, &body));
            let slots: Vec<String> = (start + 1..=params.len())
                .map(|i| format!("?{i}"))
                .collect();
            tuples.push(format!("({})", slots.join(", ")));
        }
        conn.execute(
            &format!(
                "INSERT INTO {} ({}) VALUES {}",
                table(collection_id),
                names.join(", "),
                tuples.join(", ")
            ),
            &params,
        )
        .await?;
    }
    // Building indexes over a full table is faster than updating them per row.
    create_indexes(conn, collection_id, blueprint).await
}
