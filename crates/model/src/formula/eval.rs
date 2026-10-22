//! Evaluates expressions against a document and other collections' columns.
use super::functions::function;
use super::parser::Expr;
use super::value::{compare, criterion, match_key, num, number, text_value, truth};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

/// Columns of other collections that `[collection.field]` references read.
/// Columns of one collection share a row order, so they line up like ranges.
#[derive(Clone, Default)]
pub struct Lookups {
    columns: HashMap<String, Arc<LookupColumn>>,
}

/// One field's values across a collection's records, with a lazily built
/// equality index so criteria such as `[orders.customer] = [id]` stay fast.
pub struct LookupColumn {
    collection: String,
    /// Always a `Value::Array`, so functions can borrow the whole column.
    values: Value,
    index: OnceLock<HashMap<String, Vec<usize>>>,
}

impl LookupColumn {
    pub fn new(collection: &str, values: Vec<Value>) -> Self {
        Self {
            collection: collection.into(),
            values: Value::Array(values),
            index: OnceLock::new(),
        }
    }
    fn values(&self) -> &[Value] {
        self.values
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
    fn rows(&self, key: &str) -> &[usize] {
        self.index
            .get_or_init(|| {
                let mut index = HashMap::<String, Vec<usize>>::new();
                for (row, value) in self.values().iter().enumerate() {
                    index.entry(match_key(value)).or_default().push(row);
                }
                index
            })
            .get(key)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

impl Lookups {
    pub fn insert(&mut self, collection: &str, field: &str, column: Arc<LookupColumn>) {
        self.columns.insert(format!("{collection}.{field}"), column);
    }
    fn column(&self, name: &str) -> Result<&LookupColumn, String> {
        self.columns
            .get(name)
            .map(Arc::as_ref)
            .ok_or_else(|| format!("#REF! [{name}] is read from its collection on saved records"))
    }
}

pub(super) struct Ctx<'a> {
    pub(super) all: &'a HashMap<String, Expr>,
    pub(super) body: &'a Value,
    pub(super) lookups: &'a Lookups,
}
pub(super) type Cache = HashMap<String, Result<Value, String>>;
pub(super) fn resolve(n: &str, ctx: &Ctx, cache: &mut Cache) -> Result<Value, String> {
    if let Some(v) = cache.get(n) {
        return v.clone();
    }
    let result = if let Some(e) = ctx.all.get(n) {
        eval(e, ctx, cache)
    } else if n.contains('.') {
        ctx.lookups.column(n).map(|column| column.values.clone())
    } else {
        Ok(ctx.body.get(n).cloned().unwrap_or(Value::Null))
    };
    cache.insert(n.into(), result.clone());
    result
}
pub(super) fn eval<'a>(e: &Expr, ctx: &Ctx<'a>, cache: &mut Cache) -> Result<Value, String> {
    match e {
        Expr::Literal(v) => Ok(v.clone()),
        Expr::Field(n) => resolve(n, ctx, cache),
        Expr::Unary(op, a) => {
            number(num(&eval(a, ctx, cache)?)? * if *op == '-' { -1.0 } else { 1.0 })
        }
        Expr::Binary(op, a, b) => {
            let a = eval(a, ctx, cache)?;
            let b = eval(b, ctx, cache)?;
            match op.as_str() {
                "&" => Ok(json!(text_value(&a) + &text_value(&b))),
                "=" | "<>" | "<" | ">" | "<=" | ">=" => Ok(json!(compare(&a, &b, op)?)),
                _ => {
                    let a = num(&a)?;
                    let b = num(&b)?;
                    number(match op.as_str() {
                        "+" => a + b,
                        "-" => a - b,
                        "*" => a * b,
                        "/" => {
                            if b == 0.0 {
                                return Err("#DIV/0!".into());
                            }
                            a / b
                        }
                        _ => a.powf(b),
                    })
                }
            }
        }
        Expr::Call(n, args) => {
            if n == "IF" {
                return eval(
                    &args[if truth(&eval(&args[0], ctx, cache)?)? {
                        1
                    } else {
                        2
                    }],
                    ctx,
                    cache,
                );
            }
            if n == "IFERROR" {
                return eval(&args[0], ctx, cache).or_else(|_| eval(&args[1], ctx, cache));
            }
            if matches!(
                n.as_str(),
                "SUMIFS" | "COUNTIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS"
            ) {
                return conditional(n, args, ctx, cache);
            }
            if n == "XLOOKUP" {
                return xlookup(args, ctx, cache);
            }
            let mut values = Vec::new();
            let mut bytes = 0usize;
            for a in args {
                // Another collection's column is borrowed rather than copied
                // for every record, and its size is the collection's, so it
                // does not count towards the limit on values built here.
                if let Expr::Field(name) = a
                    && name.contains('.')
                    && !ctx.all.contains_key(name)
                {
                    values.push(Cow::Borrowed(&ctx.lookups.column(name)?.values));
                    continue;
                }
                let value = eval(a, ctx, cache)?;
                bytes = bytes.saturating_add(value.to_string().len());
                if bytes > 1_000_000 {
                    return Err("#VALUE! function inputs exceed 1 MB".into());
                }
                values.push(Cow::Owned(value));
            }
            function(n, &values.iter().map(AsRef::as_ref).collect::<Vec<_>>())
        }
    }
}

/// A list argument: another collection's column is borrowed, not copied.
enum Range<'a> {
    Column(&'a LookupColumn),
    List(Vec<Value>),
}
impl Range<'_> {
    fn values(&self) -> &[Value] {
        match self {
            Range::Column(column) => column.values(),
            Range::List(values) => values,
        }
    }
}
fn range<'a>(e: &Expr, ctx: &Ctx<'a>, cache: &mut Cache) -> Result<Range<'a>, String> {
    if let Expr::Field(n) = e
        && n.contains('.')
    {
        return ctx.lookups.column(n).map(Range::Column);
    }
    Ok(match eval(e, ctx, cache)? {
        Value::Array(values) => Range::List(values),
        value => Range::List(vec![value]),
    })
}
/// Ranges must line up row by row, so columns of another collection are not
/// mixed with other lists or other collections' columns.
fn aligned(ranges: &[&Range]) -> Result<usize, String> {
    let len = ranges[0].values().len();
    if ranges.iter().any(|r| r.values().len() != len) {
        return Err("#VALUE! arrays must have equal length".into());
    }
    let collections: HashSet<Option<&str>> = ranges
        .iter()
        .map(|r| match r {
            Range::Column(column) => Some(column.collection.as_str()),
            Range::List(_) => None,
        })
        .collect();
    if collections.len() > 1 {
        return Err("#VALUE! arrays must come from the same collection".into());
    }
    Ok(len)
}
/// The criteria's equality key, when the criteria is a plain value match.
fn criteria_key(c: &Value) -> Option<String> {
    match c {
        Value::Number(_) | Value::Bool(_) => Some(match_key(c)),
        Value::String(s) => {
            let s = s.strip_prefix('=').unwrap_or(s);
            (!s.is_empty() && !s.starts_with(['<', '>', '=']) && !s.contains(['*', '?', '~']))
                .then(|| match_key(&json!(s)))
        }
        _ => None,
    }
}
/// SUMIFS, COUNTIFS, AVERAGEIFS, MINIFS and MAXIFS. Matching rows come from
/// the most selective indexed criteria, then every criteria is checked.
fn conditional(n: &str, args: &[Expr], ctx: &Ctx, cache: &mut Cache) -> Result<Value, String> {
    let (target, pairs) = if n == "COUNTIFS" {
        (None, args)
    } else {
        (Some(range(&args[0], ctx, cache)?), &args[1..])
    };
    let mut criteria = Vec::new();
    for pair in pairs.chunks(2) {
        criteria.push((range(&pair[0], ctx, cache)?, eval(&pair[1], ctx, cache)?));
    }
    let mut ranges: Vec<&Range> = criteria.iter().map(|(r, _)| r).collect();
    ranges.extend(target.as_ref());
    let len = aligned(&ranges)?;
    let mut candidates: Option<&[usize]> = None;
    for (r, c) in &criteria {
        if let (Range::Column(column), Some(key)) = (r, criteria_key(c)) {
            let rows = column.rows(&key);
            if candidates.is_none_or(|current| rows.len() < current.len()) {
                candidates = Some(rows);
            }
        }
    }
    let rows: Box<dyn Iterator<Item = usize>> = match candidates {
        Some(rows) => Box::new(rows.iter().copied()),
        None => Box::new(0..len),
    };
    let (mut count, mut numeric, mut sum) = (0usize, 0usize, 0.0);
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    'rows: for row in rows {
        for (r, c) in &criteria {
            if !criterion(&r.values()[row], c)? {
                continue 'rows;
            }
        }
        count += 1;
        if let Some(v) = target.as_ref().and_then(|t| t.values()[row].as_f64()) {
            numeric += 1;
            sum += v;
            min = min.min(v);
            max = max.max(v);
        }
    }
    match n {
        "COUNTIFS" => Ok(json!(count)),
        "SUMIFS" => number(sum),
        "AVERAGEIFS" if numeric == 0 => Err("#DIV/0!".into()),
        "AVERAGEIFS" => number(sum / numeric as f64),
        _ if numeric == 0 => number(0.0),
        "MINIFS" => number(min),
        _ => number(max),
    }
}
/// The first row whose lookup value equals `value`, by the criteria rules.
fn xlookup(args: &[Expr], ctx: &Ctx, cache: &mut Cache) -> Result<Value, String> {
    let value = eval(&args[0], ctx, cache)?;
    let lookup = range(&args[1], ctx, cache)?;
    let result = range(&args[2], ctx, cache)?;
    aligned(&[&lookup, &result])?;
    let key = match_key(&value);
    let found = match &lookup {
        Range::Column(column) => column.rows(&key).first().copied(),
        Range::List(values) => values.iter().position(|v| match_key(v) == key),
    };
    match found {
        Some(row) => Ok(result.values()[row].clone()),
        None if args.len() == 4 => eval(&args[3], ctx, cache),
        None => Err("#N/A no matching value".into()),
    }
}
