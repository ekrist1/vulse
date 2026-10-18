//! Bounded, deterministic formulas over a single document's named fields.
//! `[collection.field]` reads a whole column of another collection, like a
//! spreadsheet range on another sheet; the server supplies those as `Lookups`.
mod eval;
mod functions;
mod parser;
mod rename;
#[cfg(test)]
mod tests;
mod value;

pub use eval::{LookupColumn, Lookups};
pub use functions::call_function;
pub use parser::{Expr, formula_dependencies, inspect_formula};
pub use rename::{rename_formula_fields, rename_formula_references};
pub use value::text_value;

use crate::{Blueprint, FieldType};
use eval::{Ctx, resolve};
use parser::{parse, refs};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const FUNCTIONS: &[(&str, &str)] = &[
    ("SUM", "SUM(values…)"),
    ("AVERAGE", "AVERAGE(values…)"),
    ("MIN", "MIN(values…)"),
    ("MAX", "MAX(values…)"),
    ("COUNT", "COUNT(values…)"),
    ("COUNTA", "COUNTA(values…)"),
    ("COUNTBLANK", "COUNTBLANK(values…)"),
    ("PRODUCT", "PRODUCT(values…)"),
    ("MEDIAN", "MEDIAN(values…)"),
    ("ABS", "ABS(number)"),
    ("ROUND", "ROUND(number, digits)"),
    ("ROUNDUP", "ROUNDUP(number, digits)"),
    ("ROUNDDOWN", "ROUNDDOWN(number, digits)"),
    ("INT", "INT(number)"),
    ("MOD", "MOD(number, divisor)"),
    ("POWER", "POWER(number, power)"),
    ("SQRT", "SQRT(number)"),
    ("CEILING", "CEILING(number, significance)"),
    ("FLOOR", "FLOOR(number, significance)"),
    ("IF", "IF(condition, yes, no)"),
    ("IFERROR", "IFERROR(value, fallback)"),
    ("AND", "AND(values…)"),
    ("OR", "OR(values…)"),
    ("NOT", "NOT(value)"),
    ("CONCAT", "CONCAT(values…)"),
    ("CONCATENATE", "CONCATENATE(values…)"),
    ("TEXTJOIN", "TEXTJOIN(delimiter, ignore_empty, values…)"),
    ("LEFT", "LEFT(text, count)"),
    ("RIGHT", "RIGHT(text, count)"),
    ("MID", "MID(text, start, count)"),
    ("LEN", "LEN(text)"),
    ("LOWER", "LOWER(text)"),
    ("UPPER", "UPPER(text)"),
    ("PROPER", "PROPER(text)"),
    ("TRIM", "TRIM(text)"),
    ("SUBSTITUTE", "SUBSTITUTE(text, old, new, occurrence?)"),
    ("REPLACE", "REPLACE(text, start, count, replacement)"),
    ("FIND", "FIND(needle, text, start?)"),
    ("SEARCH", "SEARCH(needle, text, start?)"),
    ("EXACT", "EXACT(a, b)"),
    ("VALUE", "VALUE(text)"),
    ("DATE", "DATE(year, month, day)"),
    ("YEAR", "YEAR(date)"),
    ("MONTH", "MONTH(date)"),
    ("DAY", "DAY(date)"),
    ("SUMIF", "SUMIF(array, criteria, sum_array?)"),
    ("COUNTIF", "COUNTIF(array, criteria)"),
    ("AVERAGEIF", "AVERAGEIF(array, criteria, average_array?)"),
    ("SUMIFS", "SUMIFS(sum_array, criteria_array, criteria, …)"),
    ("COUNTIFS", "COUNTIFS(criteria_array, criteria, …)"),
    (
        "AVERAGEIFS",
        "AVERAGEIFS(average_array, criteria_array, criteria, …)",
    ),
    ("MINIFS", "MINIFS(min_array, criteria_array, criteria, …)"),
    ("MAXIFS", "MAXIFS(max_array, criteria_array, criteria, …)"),
    (
        "XLOOKUP",
        "XLOOKUP(value, lookup_array, return_array, if_not_found?)",
    ),
    ("INDEX", "INDEX(array, position)"),
    ("MATCH", "MATCH(value, array, mode?)"),
];

fn compile(bp: &Blueprint) -> Result<HashMap<String, Expr>, String> {
    let mut expressions = HashMap::new();
    for f in &bp.fields {
        if f.kind == FieldType::Calculated {
            let e = parse(
                f.formula
                    .as_deref()
                    .ok_or_else(|| format!("{} needs a formula", f.name))?,
            )
            .map_err(|e| format!("{}: {e}", f.name))?;
            let mut dependencies = HashSet::new();
            refs(&e, &mut dependencies);
            for n in dependencies {
                if let Some((collection, field)) = n.split_once('.') {
                    if crate::check_collection_name(collection).is_err()
                        || !crate::identity::is_field_name(field)
                    {
                        return Err(format!(
                            "{}: [{n}] must name a collection and a field, like [orders.amount]",
                            f.name
                        ));
                    }
                    continue;
                }
                if !bp.fields.iter().any(|f| f.name == n) {
                    return Err(format!("{}: unknown field [{n}]", f.name));
                }
            }
            expressions.insert(f.name.clone(), e);
        }
    }
    fn visit(
        n: &str,
        all: &HashMap<String, Expr>,
        active: &mut HashSet<String>,
        done: &mut HashSet<String>,
    ) -> Result<(), String> {
        if done.contains(n) {
            return Ok(());
        }
        if !active.insert(n.into()) {
            return Err(format!("circular formula dependency at [{n}]"));
        }
        if active.len() > 64 {
            return Err("calculated dependency chain exceeds 64".into());
        }
        if let Some(e) = all.get(n) {
            let mut deps = HashSet::new();
            refs(e, &mut deps);
            for d in deps {
                if all.contains_key(&d) {
                    visit(&d, all, active, done)?;
                }
            }
        }
        active.remove(n);
        done.insert(n.into());
        Ok(())
    }
    let mut done = HashSet::new();
    for n in expressions.keys() {
        visit(n, &expressions, &mut HashSet::new(), &mut done)?;
    }
    Ok(expressions)
}
pub fn validate_formulas(bp: &Blueprint) -> Result<(), String> {
    compile(bp).map(|_| ())
}

/// The `(collection, field)` columns a formula reads from other collections.
pub fn formula_lookups(formula: &str) -> Vec<(String, String)> {
    let mut names = HashSet::new();
    if let Ok(e) = parse(formula) {
        refs(&e, &mut names);
    }
    let found: BTreeSet<_> = names
        .iter()
        .filter_map(|n| n.split_once('.'))
        .map(|(c, f)| (c.to_string(), f.to_string()))
        .collect();
    found.into_iter().collect()
}

/// Every column the calculated fields of a blueprint read from collections.
pub fn lookup_references(bp: &Blueprint) -> Vec<(String, String)> {
    let found: BTreeSet<_> = bp
        .fields
        .iter()
        .filter(|f| f.kind == FieldType::Calculated)
        .filter_map(|f| f.formula.as_deref())
        .flat_map(formula_lookups)
        .collect();
    found.into_iter().collect()
}

/// Calculates fresh values, ignoring client-supplied calculated values. Errors are kept separate.
/// References to other collections fail; see `calculate_document_with`.
pub fn calculate_document(bp: &Blueprint, body: &Value) -> (Value, BTreeMap<String, String>) {
    calculate_document_with(bp, body, &Lookups::default())
}

/// Like `calculate_document`, reading `[collection.field]` columns from `lookups`.
pub fn calculate_document_with(
    bp: &Blueprint,
    body: &Value,
    lookups: &Lookups,
) -> (Value, BTreeMap<String, String>) {
    let mut out = body.as_object().cloned().unwrap_or_default();
    let mut errors = BTreeMap::new();
    let expressions = match compile(bp) {
        Ok(e) => e,
        Err(e) => {
            errors.insert("blueprint".into(), e);
            return (Value::Object(out), errors);
        }
    };
    let ctx = Ctx {
        all: &expressions,
        body,
        lookups,
    };
    let mut cache = HashMap::new();
    for n in expressions.keys() {
        match resolve(n, &ctx, &mut cache) {
            Ok(v) => {
                out.insert(n.clone(), v);
            }
            Err(e) => {
                out.insert(n.clone(), Value::Null);
                errors.insert(n.clone(), e);
            }
        }
    }
    (Value::Object(out), errors)
}
pub fn source_document(bp: &Blueprint, body: &Value) -> Value {
    let mut out = body.clone();
    if let Some(obj) = out.as_object_mut() {
        for f in &bp.fields {
            if f.kind == FieldType::Calculated {
                obj.remove(&f.name);
            }
        }
    }
    out
}
