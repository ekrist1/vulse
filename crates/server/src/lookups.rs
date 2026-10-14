//! Columns of other collections for `[collection.field]` formula references.
//!
//! A column is computed from the collection's record bodies, in creation
//! order, and cached until the collection's records or fields change.
//! Columns may hold calculated values, but not ones that read other
//! collections themselves, so lookups never chain.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use vulse_model::{
    Blueprint, CollectionView, FieldType, LookupColumn, Lookups, formula_lookups, lookup_references,
};

use crate::db::{Conn, SqlValue, int_at, text_at};
use crate::error::AppError;
use crate::store;

pub(crate) struct Cached {
    fingerprint: String,
    columns: HashMap<String, Arc<LookupColumn>>,
}

/// Cached columns by collection id.
pub(crate) type Cache = Mutex<HashMap<String, Cached>>;
static CACHE: LazyLock<Cache> = LazyLock::new(Default::default);

pub(crate) fn invalidate(collection_id: &str) {
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(collection_id);
}

/// The columns that `blueprint`'s formulas read from collections.
pub(crate) async fn load(conn: &Conn, blueprint: &Blueprint) -> anyhow::Result<Lookups> {
    let mut wanted = BTreeMap::<String, Vec<String>>::new();
    for (collection, field) in lookup_references(blueprint) {
        wanted.entry(collection).or_default().push(field);
    }
    let mut lookups = Lookups::default();
    for (name, fields) in wanted {
        // A missing collection or field leaves the reference unresolved, and
        // the formula reports it.
        let Some(collection) = store::find_collection_by_name(conn, &name).await? else {
            continue;
        };
        for (field, column) in columns(conn, &collection, &fields).await? {
            lookups.insert(&name, &field, column);
        }
    }
    Ok(lookups)
}

async fn columns(
    conn: &Conn,
    collection: &CollectionView,
    fields: &[String],
) -> anyhow::Result<Vec<(String, Arc<LookupColumn>)>> {
    // A transaction sees its own writes (or an older read snapshot). Neither
    // may reuse or populate the cache shared with other requests.
    let cache = if conn.in_transaction() {
        &conn.lookup_cache
    } else {
        &CACHE
    };
    let rows = conn
        .query(
            "SELECT COUNT(*), COALESCE(MAX(updated_at), '') FROM documents WHERE collection_id=?1",
            &[SqlValue::Text(collection.id.clone())],
        )
        .await?;
    let row = rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing count"))?;
    let fingerprint = format!(
        "{}|{}|{}",
        int_at(row, 0)?,
        text_at(row, 1)?,
        collection.updated_at
    );
    let fields: Vec<&String> = fields
        .iter()
        .filter(|f| collection.blueprint.fields.iter().any(|d| d.name == **f))
        // Older schemas may contain indirect chains accepted before validation
        // followed local dependencies. Leave these unresolved, so callers get
        // a calculation error rather than aggregating fabricated null values.
        .filter(|f| !reads_collections(&collection.blueprint, f))
        .collect();
    {
        let cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = cache
            .get(&collection.id)
            .filter(|c| c.fingerprint == fingerprint)
            && let Some(found) = fields
                .iter()
                .map(|f| cached.columns.get(*f).map(|c| ((*f).clone(), c.clone())))
                .collect::<Option<Vec<_>>>()
        {
            return Ok(found);
        }
    }
    let rows = conn
        .query(
            "SELECT body FROM documents WHERE collection_id=?1 ORDER BY created_at, id",
            &[SqlValue::Text(collection.id.clone())],
        )
        .await?;
    let calculated = fields.iter().any(|f| {
        collection
            .blueprint
            .fields
            .iter()
            .any(|d| d.name == **f && d.kind == FieldType::Calculated)
    });
    let mut values: Vec<Vec<serde_json::Value>> =
        vec![Vec::with_capacity(rows.len()); fields.len()];
    for row in &rows {
        let mut body: serde_json::Value = serde_json::from_str(&text_at(row, 0)?)?;
        if calculated {
            body = vulse_model::calculate_document(&collection.blueprint, &body).0;
        }
        for (index, field) in fields.iter().enumerate() {
            values[index].push(body.get(*field).cloned().unwrap_or_default());
        }
    }
    let found: Vec<(String, Arc<LookupColumn>)> = fields
        .iter()
        .zip(values)
        .map(|(field, values)| {
            (
                (*field).clone(),
                Arc::new(LookupColumn::new(&collection.name, values)),
            )
        })
        .collect();
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let entry = cache
        .entry(collection.id.clone())
        .or_insert_with(|| Cached {
            fingerprint: fingerprint.clone(),
            columns: HashMap::new(),
        });
    if entry.fingerprint != fingerprint {
        entry.fingerprint = fingerprint;
        entry.columns.clear();
    }
    for (field, column) in &found {
        entry.columns.insert(field.clone(), column.clone());
    }
    Ok(found)
}

/// Whether a field reads other collections, which makes it unusable as a
/// lookup column itself.
pub(crate) fn reads_collections(blueprint: &Blueprint, field: &str) -> bool {
    let mut pending = vec![field.to_string()];
    let mut visited = std::collections::HashSet::new();
    while let Some(name) = pending.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        let Some(formula) = blueprint
            .fields
            .iter()
            .find(|f| f.name == name && f.kind == FieldType::Calculated)
            .and_then(|f| f.formula.as_deref())
        else {
            continue;
        };
        if !formula_lookups(formula).is_empty() {
            return true;
        }
        if let Ok(dependencies) = vulse_model::formula_dependencies(formula) {
            pending.extend(dependencies);
        }
    }
    false
}

/// An existing collection being changed: its id, its current name and its
/// field renames (old name → new name).
pub(crate) struct Previous<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub renames: &'a BTreeMap<String, String>,
}

/// Checks the references of a blueprint about to be saved as collection
/// `name`: each must name an existing field, and no lookup may chain. Other
/// collections' formulas reading this one must still find their fields.
pub(crate) async fn check(
    conn: &Conn,
    previous: Option<Previous<'_>>,
    name: &str,
    blueprint: &Blueprint,
) -> Result<(), AppError> {
    for field in &blueprint.fields {
        let Some(formula) = field.formula.as_deref() else {
            continue;
        };
        if field.kind != FieldType::Calculated {
            continue;
        }
        for (collection, target) in formula_lookups(formula) {
            let other;
            let source = if collection == name {
                blueprint
            } else {
                other = store::find_collection_by_name(conn, &collection)
                    .await?
                    .ok_or_else(|| {
                        AppError::bad(format!(
                            "{}: unknown collection '{collection}' in [{collection}.{target}]",
                            field.name
                        ))
                    })?;
                &other.blueprint
            };
            if !source.fields.iter().any(|f| f.name == target) {
                return Err(AppError::bad(format!(
                    "{}: collection '{collection}' has no field '{target}'",
                    field.name
                )));
            }
            if reads_collections(source, &target) {
                return Err(AppError::bad(format!(
                    "{}: [{collection}.{target}] reads other collections itself. A formula can only read fields that don't.",
                    field.name
                )));
            }
        }
    }
    let Some(previous) = previous else {
        return Ok(());
    };
    for other in crate::relationships::all_collections(conn).await? {
        if other.id == previous.id {
            continue;
        }
        for field in &other.blueprint.fields {
            if field.kind != FieldType::Calculated {
                continue;
            }
            for (collection, target) in
                formula_lookups(field.formula.as_deref().unwrap_or_default())
            {
                if collection != previous.name {
                    continue;
                }
                let target = previous.renames.get(&target).cloned().unwrap_or(target);
                if !blueprint.fields.iter().any(|f| f.name == target) {
                    return Err(AppError::conflict(format!(
                        "The formula of '{}.{}' uses field '{target}'. Change that formula first.",
                        other.name, field.name
                    )));
                }
                if reads_collections(blueprint, &target) {
                    return Err(AppError::bad(format!(
                        "'{target}' cannot read other collections, because the formula of '{}.{}' reads it.",
                        other.name, field.name
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Rewrites `[old_name.field]` references after a collection or field rename.
pub(crate) fn rename(
    blueprint: &mut Blueprint,
    old_name: &str,
    new_name: &str,
    renames: &BTreeMap<String, String>,
) -> bool {
    let mut changed = false;
    for field in &mut blueprint.fields {
        let Some(formula) = field.formula.as_deref() else {
            continue;
        };
        let next = vulse_model::rename_formula_references(formula, |reference| {
            let (collection, target) = reference.split_once('.')?;
            (collection == old_name).then(|| {
                format!(
                    "{new_name}.{}",
                    renames.get(target).map(String::as_str).unwrap_or(target)
                )
            })
        });
        if next != formula {
            field.formula = Some(next);
            changed = true;
        }
    }
    changed
}

/// Refuses to delete a collection that other collections' formulas read.
pub(crate) async fn check_delete(conn: &Conn, id: &str, name: &str) -> Result<(), AppError> {
    for other in crate::relationships::all_collections(conn).await? {
        if other.id != id
            && lookup_references(&other.blueprint)
                .iter()
                .any(|(c, _)| c == name)
        {
            return Err(AppError::conflict(format!(
                "Formulas in collection '{}' read this collection. Change those formulas first.",
                other.name
            )));
        }
    }
    Ok(())
}
