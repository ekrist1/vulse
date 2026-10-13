//! Bounded, deterministic formulas over a single document's named fields.
//! `[collection.field]` reads a whole column of another collection, like a
//! spreadsheet range on another sheet; the server supplies those as `Lookups`.
use crate::{Blueprint, FieldType};
use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, OnceLock};

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

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Literal(Value),
    Field(String),
    Unary(char, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}
struct Parser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
}
impl Parser {
    fn ws(&mut self) {
        while self.chars.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }
    fn take(&mut self, c: char) -> bool {
        self.ws();
        if self.chars.get(self.pos) == Some(&c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expr(&mut self, min: u8) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("formula nesting exceeds 64".into());
        }
        let mut left = self.atom()?;
        loop {
            self.ws();
            let rest: String = self.chars[self.pos..].iter().take(2).collect();
            let op = [
                "<=", ">=", "<>", "=", "<", ">", "&", "+", "-", "*", "/", "^",
            ]
            .into_iter()
            .find(|op| rest.starts_with(op));
            let Some(op) = op else { break };
            let prec = match op {
                "=" | "<>" | "<" | ">" | "<=" | ">=" => 1,
                "&" => 2,
                "+" | "-" => 3,
                "*" | "/" => 4,
                _ => 5,
            };
            if prec < min {
                break;
            }
            self.pos += op.len();
            let right = self.expr(prec + 1)?;
            left = Expr::Binary(op.into(), Box::new(left), Box::new(right));
        }
        self.depth -= 1;
        Ok(left)
    }
    fn atom(&mut self) -> Result<Expr, String> {
        self.ws();
        if self.take('+') {
            return Ok(Expr::Unary('+', Box::new(self.expr(6)?)));
        }
        if self.take('-') {
            return Ok(Expr::Unary('-', Box::new(self.expr(6)?)));
        }
        if self.take('(') {
            let e = self.expr(1)?;
            if !self.take(')') {
                return Err("expected ')'".into());
            }
            return Ok(e);
        }
        if self.take('[') {
            let start = self.pos;
            while self.pos < self.chars.len() && self.chars[self.pos] != ']' {
                self.pos += 1;
            }
            let name: String = self.chars[start..self.pos].iter().collect();
            if !self.take(']') || name.is_empty() {
                return Err("expected [field_name]".into());
            }
            return Ok(Expr::Field(name));
        }
        if self.take('"') {
            let mut s = String::new();
            loop {
                let c = *self.chars.get(self.pos).ok_or("unclosed string")?;
                self.pos += 1;
                if c == '"' {
                    if self.chars.get(self.pos) == Some(&'"') {
                        self.pos += 1;
                        s.push('"');
                    } else {
                        break;
                    }
                } else {
                    s.push(c);
                }
            }
            return Ok(Expr::Literal(Value::String(s)));
        }
        let start = self.pos;
        if self
            .chars
            .get(self.pos)
            .is_some_and(|c| c.is_ascii_digit() || *c == '.')
        {
            while self
                .chars
                .get(self.pos)
                .is_some_and(|c| c.is_ascii_digit() || *c == '.')
            {
                self.pos += 1;
            }
            if self
                .chars
                .get(self.pos)
                .is_some_and(|c| *c == 'e' || *c == 'E')
            {
                self.pos += 1;
                if self
                    .chars
                    .get(self.pos)
                    .is_some_and(|c| *c == '+' || *c == '-')
                {
                    self.pos += 1;
                }
                while self.chars.get(self.pos).is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
            let s: String = self.chars[start..self.pos].iter().collect();
            return Ok(Expr::Literal(number(
                s.parse().map_err(|_| "invalid number")?,
            )?));
        }
        while self
            .chars
            .get(self.pos)
            .is_some_and(|c| c.is_ascii_alphabetic())
        {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(format!("expected a value at character {}", self.pos + 1));
        }
        let name: String = self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .to_ascii_uppercase();
        if name == "TRUE" || name == "FALSE" {
            return Ok(Expr::Literal(json!(name == "TRUE")));
        }
        if !FUNCTIONS.iter().any(|(n, _)| *n == name) {
            return Err(format!("unsupported function '{name}'"));
        }
        if !self.take('(') {
            return Err("expected '(' after function".into());
        }
        let mut args = vec![];
        if !self.take(')') {
            loop {
                args.push(self.expr(1)?);
                if self.take(')') {
                    break;
                }
                // Spreadsheets in many locales separate arguments with ';'.
                if !self.take(',') && !self.take(';') {
                    return Err("expected ',' or ')'".into());
                }
            }
        }
        arity(&name, args.len())?;
        Ok(Expr::Call(name, args))
    }
}
fn parse(formula: &str) -> Result<Expr, String> {
    if formula.len() > 4096 {
        return Err("formula exceeds 4096 bytes".into());
    }
    let mut p = Parser {
        chars: formula.trim().trim_start_matches('=').chars().collect(),
        pos: 0,
        depth: 0,
    };
    let e = p.expr(1)?;
    p.ws();
    if p.pos != p.chars.len() {
        return Err(format!("unexpected character at {}", p.pos + 1));
    }
    let mut pending = vec![(&e, 0usize)];
    let mut nodes = 0;
    while let Some((expr, depth)) = pending.pop() {
        nodes += 1;
        if depth > 64 || nodes > 512 {
            return Err("Formula exceeds 64 levels or 512 operations".into());
        }
        match expr {
            Expr::Unary(_, a) => pending.push((a, depth + 1)),
            Expr::Binary(_, a, b) => {
                pending.push((a, depth + 1));
                pending.push((b, depth + 1));
            }
            Expr::Call(_, args) => {
                for a in args {
                    pending.push((a, depth + 1));
                }
            }
            _ => {}
        }
    }
    Ok(e)
}
/// Inspect a bounded expression without evaluating it, for guided editing and dependency previews.
pub fn inspect_formula(formula: &str) -> Result<Expr, String> {
    parse(formula)
}

pub fn formula_dependencies(formula: &str) -> Result<Vec<String>, String> {
    let mut names = HashSet::new();
    refs(&parse(formula)?, &mut names);
    let mut names: Vec<_> = names.into_iter().collect();
    names.sort();
    Ok(names)
}

fn arity(name: &str, n: usize) -> Result<(), String> {
    let (min, max) = match name {
        "SUM" | "AVERAGE" | "MIN" | "MAX" | "COUNT" | "COUNTA" | "COUNTBLANK" | "PRODUCT"
        | "MEDIAN" | "AND" | "OR" | "CONCAT" | "CONCATENATE" => (1, 255),
        "TEXTJOIN" => (3, 255),
        "IF" | "MID" | "DATE" => (3, 3),
        "SUBSTITUTE" => (3, 4),
        "REPLACE" => (4, 4),
        "LEFT" | "RIGHT" => (1, 2),
        "FIND" | "SEARCH" | "SUMIF" | "AVERAGEIF" | "MATCH" => (2, 3),
        "IFERROR" | "ROUND" | "ROUNDUP" | "ROUNDDOWN" | "MOD" | "POWER" | "CEILING" | "FLOOR"
        | "EXACT" | "COUNTIF" | "INDEX" => (2, 2),
        "SUMIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS" => (3, 255),
        "COUNTIFS" => (2, 254),
        "XLOOKUP" => (3, 4),
        _ => (1, 1),
    };
    let pairs = match name {
        "SUMIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS" => !n.is_multiple_of(2),
        "COUNTIFS" => n.is_multiple_of(2),
        _ => true,
    };
    if !pairs {
        return Err(format!(
            "{name} expects each criteria array to be followed by its criteria"
        ));
    }
    if n < min || n > max {
        Err(format!(
            "{name} expects {min}–{max} arguments, received {n}"
        ))
    } else {
        Ok(())
    }
}
fn refs(e: &Expr, out: &mut HashSet<String>) {
    match e {
        Expr::Field(n) => {
            out.insert(n.clone());
        }
        Expr::Unary(_, a) => refs(a, out),
        Expr::Binary(_, a, b) => {
            refs(a, out);
            refs(b, out);
        }
        Expr::Call(_, args) => {
            for a in args {
                refs(a, out);
            }
        }
        _ => {}
    }
}
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
    values: Vec<Value>,
    index: OnceLock<HashMap<String, Vec<usize>>>,
}

impl LookupColumn {
    pub fn new(collection: &str, values: Vec<Value>) -> Self {
        Self {
            collection: collection.into(),
            values,
            index: OnceLock::new(),
        }
    }
    fn rows(&self, key: &str) -> &[usize] {
        self.index
            .get_or_init(|| {
                let mut index = HashMap::<String, Vec<usize>>::new();
                for (row, value) in self.values.iter().enumerate() {
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
struct Ctx<'a> {
    all: &'a HashMap<String, Expr>,
    body: &'a Value,
    lookups: &'a Lookups,
}
type Cache = HashMap<String, Result<Value, String>>;
fn resolve(n: &str, ctx: &Ctx, cache: &mut Cache) -> Result<Value, String> {
    if let Some(v) = cache.get(n) {
        return v.clone();
    }
    let result = if let Some(e) = ctx.all.get(n) {
        eval(e, ctx, cache)
    } else if n.contains('.') {
        ctx.lookups
            .column(n)
            .map(|column| Value::Array(column.values.clone()))
    } else {
        Ok(ctx.body.get(n).cloned().unwrap_or(Value::Null))
    };
    cache.insert(n.into(), result.clone());
    result
}
fn eval(e: &Expr, ctx: &Ctx, cache: &mut Cache) -> Result<Value, String> {
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
                let value = eval(a, ctx, cache)?;
                bytes = bytes.saturating_add(value.to_string().len());
                if bytes > 1_000_000 {
                    return Err("#VALUE! function inputs exceed 1 MB".into());
                }
                values.push(value);
            }
            function(n, &values)
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
            Range::Column(column) => &column.values,
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
/// Values that compare equal in criteria share a key: numbers and numeric
/// text by value, other text case-insensitively.
fn match_key(v: &Value) -> String {
    match as_number(v) {
        // -0 and 0 are the same value.
        Some(n) => if n == 0.0 { 0.0 } else { n }.to_string(),
        None => text_value(v).to_lowercase(),
    }
}
fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
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
fn number(n: f64) -> Result<Value, String> {
    serde_json::Number::from_f64(n)
        .map(Value::Number)
        .ok_or("#NUM!".into())
}
fn num(v: &Value) -> Result<f64, String> {
    match v {
        Value::Null => Ok(0.0),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64().ok_or("#NUM!".into()),
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or("#VALUE! expected number".into()),
        _ => Err("#VALUE! expected number".into()),
    }
}
fn truth(v: &Value) -> Result<bool, String> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ => Ok(num(v)? != 0.0),
    }
}
pub fn text_value(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => {
            if *b {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        _ => v.to_string(),
    }
}
fn flat(v: &Value, out: &mut Vec<Value>) {
    if let Value::Array(a) = v {
        for v in a {
            flat(v, out);
        }
    } else {
        out.push(v.clone());
    }
}
fn compare(a: &Value, b: &Value, op: &str) -> Result<bool, String> {
    let ord = if a.is_number() && b.is_number() {
        num(a)?.partial_cmp(&num(b)?).ok_or("#NUM!")?
    } else {
        text_value(a)
            .to_lowercase()
            .cmp(&text_value(b).to_lowercase())
    };
    use std::cmp::Ordering::*;
    Ok(match op {
        "=" => ord == Equal,
        "<>" => ord != Equal,
        "<" => ord == Less,
        ">" => ord == Greater,
        "<=" => ord != Greater,
        _ => ord != Less,
    })
}
fn index(v: &Value, min: usize) -> Result<usize, String> {
    let n = num(v)?;
    if n < min as f64 || n > 1_000_000.0 {
        return Err("#VALUE! index out of range".into());
    }
    Ok(n.trunc() as usize)
}
fn date(v: &Value) -> Result<NaiveDate, String> {
    if let Some(s) = v.as_str() {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| "#VALUE! expected YYYY-MM-DD".into())
    } else {
        let n = num(v)?;
        if !(1.0..=2_958_465.0).contains(&n) {
            return Err("#NUM! date out of range".into());
        }
        if n.trunc() == 60.0 {
            return Err("#NUM! Excel's fictitious 1900-02-29 is unsupported".into());
        }
        NaiveDate::from_ymd_opt(1899, 12, 31)
            .unwrap()
            .checked_add_signed(Duration::days(
                n.trunc() as i64 - if n >= 60.0 { 1 } else { 0 },
            ))
            .ok_or("#NUM!".into())
    }
}
fn criterion(v: &Value, c: &Value) -> Result<bool, String> {
    let s = text_value(c);
    let op = ["<=", ">=", "<>", "<", ">", "="]
        .into_iter()
        .find(|o| s.starts_with(o))
        .unwrap_or("=");
    let rhs = s.strip_prefix(op).unwrap_or(&s);
    let b = rhs
        .parse::<f64>()
        .ok()
        .and_then(|n| number(n).ok())
        .unwrap_or(json!(rhs));
    if (op == "=" || op == "<>") && !b.is_number() && rhs.contains(['*', '?', '~']) {
        let matched = wildcard(&text_value(v).to_lowercase(), &rhs.to_lowercase(), false)?;
        return Ok(if op == "<>" { !matched } else { matched });
    }
    // Numeric text matches numeric criteria, as in spreadsheets.
    if b.is_number()
        && let Some(n) = as_number(v)
    {
        return compare(&json!(n), &b, op);
    }
    compare(v, &b, op)
}
/// Calls a formula function on already evaluated arguments, for SQL
/// expressions. `IF` and `IFERROR` evaluate eagerly; functions that read
/// lists of other collections are not available.
pub fn call_function(name: &str, args: &[Value]) -> Result<Value, String> {
    let name = name.to_ascii_uppercase();
    if !FUNCTIONS.iter().any(|(n, _)| *n == name) {
        return Err(format!("unknown function {name}"));
    }
    arity(&name, args.len())?;
    match name.as_str() {
        "IF" => Ok(args[if truth(&args[0])? { 1 } else { 2 }].clone()),
        "IFERROR" => Ok(args[0].clone()),
        "SUMIFS" | "COUNTIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS" | "XLOOKUP" => {
            Err(format!("{name} is not available in SQL; use WHERE and GROUP BY"))
        }
        _ => function(&name, args),
    }
}
fn function(n: &str, a: &[Value]) -> Result<Value, String> {
    let mut values = vec![];
    for v in a {
        flat(v, &mut values);
    }
    let nums: Vec<f64> = values.iter().filter_map(Value::as_f64).collect();
    let first = || num(&a[0]);
    let second = || num(&a[1]);
    let s = || text_value(&a[0]);
    match n {
        "SUM" => number(nums.iter().sum()),
        "PRODUCT" => number(if nums.is_empty() {
            0.0
        } else {
            nums.iter().product()
        }),
        "COUNT" => Ok(json!(nums.len())),
        "COUNTA" => Ok(json!(values.iter().filter(|v| !v.is_null()).count())),
        "COUNTBLANK" => Ok(json!(
            values
                .iter()
                .filter(|v| v.is_null() || **v == json!(""))
                .count()
        )),
        "AVERAGE" | "MEDIAN" => {
            if nums.is_empty() {
                return Err("#DIV/0!".into());
            }
            if n == "AVERAGE" {
                number(nums.iter().sum::<f64>() / nums.len() as f64)
            } else {
                let mut nums = nums;
                nums.sort_by(f64::total_cmp);
                let m = nums.len() / 2;
                number(if nums.len().is_multiple_of(2) {
                    (nums[m - 1] + nums[m]) / 2.0
                } else {
                    nums[m]
                })
            }
        }
        "MIN" => number(nums.iter().copied().reduce(f64::min).unwrap_or(0.0)),
        "MAX" => number(nums.iter().copied().reduce(f64::max).unwrap_or(0.0)),
        "ABS" => number(first()?.abs()),
        "INT" => number(first()?.floor()),
        "SQRT" => number(first()?.sqrt()),
        "POWER" => number(first()?.powf(second()?)),
        "MOD" => {
            let b = second()?;
            if b == 0.0 {
                return Err("#DIV/0!".into());
            }
            number(first()? - b * (first()? / b).floor())
        }
        "ROUND" | "ROUNDUP" | "ROUNDDOWN" => {
            let d = second()?.trunc();
            if !(-308.0..=308.0).contains(&d) {
                return Err("#NUM! digits out of range".into());
            }
            let f = 10f64.powf(d);
            let x = first()? * f;
            number(match n {
                "ROUND" => x.round() / f,
                "ROUNDUP" => x.signum() * x.abs().ceil() / f,
                _ => x.trunc() / f,
            })
        }
        "CEILING" | "FLOOR" => {
            let x = first()?;
            let step = second()?;
            if step == 0.0 {
                return number(0.0);
            }
            if x > 0.0 && step < 0.0 {
                return Err("#NUM!".into());
            }
            let q = x / step;
            number(if n == "CEILING" {
                q.ceil() * step
            } else {
                q.floor() * step
            })
        }
        "AND" => Ok(json!(
            values
                .iter()
                .map(truth)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .all(|b| b)
        )),
        "OR" => Ok(json!(
            values
                .iter()
                .map(truth)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .any(|b| b)
        )),
        "NOT" => Ok(json!(!truth(&a[0])?)),
        "CONCAT" | "CONCATENATE" => join_text(values.iter().map(text_value), ""),
        "TEXTJOIN" => {
            let mut v = vec![];
            for value in &a[2..] {
                flat(value, &mut v);
            }
            let ignore = truth(&a[1])?;
            join_text(
                v.iter()
                    .map(text_value)
                    .filter(|s| !ignore || !s.is_empty()),
                &s(),
            )
        }
        "LEN" => Ok(json!(s().chars().count())),
        "LOWER" => bounded_text(s().to_lowercase()),
        "UPPER" => bounded_text(s().to_uppercase()),
        "TRIM" => bounded_text(
            s().split(' ')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        ),
        "PROPER" => {
            let mut start = true;
            let mut out = String::new();
            for c in s().chars() {
                if start {
                    out.extend(c.to_uppercase());
                } else {
                    out.extend(c.to_lowercase());
                }
                start = !c.is_alphabetic();
            }
            bounded_text(out)
        }
        "LEFT" | "RIGHT" => {
            let chars: Vec<char> = s().chars().collect();
            let count = if a.len() == 1 { 1 } else { index(&a[1], 0)? }.min(chars.len());
            bounded_text(if n == "LEFT" {
                chars[..count].iter().collect()
            } else {
                chars[chars.len() - count..].iter().collect()
            })
        }
        "MID" => bounded_text(
            s().chars()
                .skip(index(&a[1], 1)? - 1)
                .take(index(&a[2], 0)?)
                .collect(),
        ),
        "REPLACE" => {
            let chars: Vec<char> = s().chars().collect();
            let start = (index(&a[1], 1)? - 1).min(chars.len());
            let end = (start + index(&a[2], 0)?).min(chars.len());
            bounded_text(
                chars[..start].iter().collect::<String>()
                    + &text_value(&a[3])
                    + &chars[end..].iter().collect::<String>(),
            )
        }
        "SUBSTITUTE" => {
            let old = text_value(&a[1]);
            let new = text_value(&a[2]);
            let text = s();
            if old.is_empty() {
                return bounded_text(text);
            }
            let occurrences = text.matches(&old).count();
            let replacements = if a.len() == 3 {
                occurrences
            } else {
                usize::from(index(&a[3], 1)? <= occurrences)
            };
            let projected = text
                .len()
                .saturating_sub(replacements.saturating_mul(old.len()))
                .saturating_add(replacements.saturating_mul(new.len()));
            if projected > 1_000_000 {
                return Err("#VALUE! result exceeds 1 MB".into());
            }
            if a.len() == 3 {
                bounded_text(text.replace(&old, &new))
            } else {
                let occurrence = index(&a[3], 1)?;
                let mut count = 0;
                let mut out = String::new();
                let mut rest = text.as_str();
                while let Some(pos) = rest.find(&old) {
                    count += 1;
                    out.push_str(&rest[..pos]);
                    out.push_str(if count == occurrence { &new } else { &old });
                    rest = &rest[pos + old.len()..];
                }
                out.push_str(rest);
                bounded_text(out)
            }
        }
        "FIND" | "SEARCH" => {
            let needle = s();
            let text = text_value(&a[1]);
            let start = if a.len() == 3 {
                index(&a[2], 1)? - 1
            } else {
                0
            };
            if start > text.chars().count() {
                return Err("#VALUE! start exceeds text length".into());
            }
            let tail: String = text.chars().skip(start).collect();
            let (needle, tail) = if n == "SEARCH" {
                (needle.to_lowercase(), tail.to_lowercase())
            } else {
                (needle, tail)
            };
            if n == "SEARCH" && needle.contains(['*', '?', '~']) {
                if tail.chars().count() > 32767 {
                    return Err("#VALUE! SEARCH text exceeds 32767 characters".into());
                }
                let chars: Vec<char> = tail.chars().collect();
                let mut budget = 1_000_000usize;
                for i in 0..=chars.len() {
                    if wildcard_chars(&chars[i..], &needle, true, &mut budget)? {
                        return Ok(json!(start + i + 1));
                    }
                }
                return Err("#VALUE! text not found".into());
            }
            let p = tail.find(&needle).ok_or("#VALUE! text not found")?;
            Ok(json!(start + tail[..p].chars().count() + 1))
        }
        "EXACT" => Ok(json!(s() == text_value(&a[1]))),
        "VALUE" => number(first()?),
        "DATE" => {
            let mut y = first()?.trunc() as i32;
            let m = second()?.trunc() as i32;
            let d = num(&a[2])?.trunc() as i64;
            if (0..1900).contains(&y) {
                y += 1900;
            }
            if !(1..=9999).contains(&y)
                || !(-120_000..=120_000).contains(&m)
                || d.unsigned_abs() > 3_652_058
            {
                return Err("#NUM! date out of range".into());
            }
            let total = y * 12 + m - 1;
            let date =
                NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1)
                    .and_then(|d0| d0.checked_add_signed(Duration::days(d - 1)))
                    .ok_or("#NUM! invalid date")?;
            if !(1900..=9999).contains(&date.year()) {
                return Err("#NUM! date out of range".into());
            }
            let mut serial = (date - NaiveDate::from_ymd_opt(1899, 12, 31).unwrap()).num_days();
            if date >= NaiveDate::from_ymd_opt(1900, 3, 1).unwrap() {
                serial += 1;
            }
            Ok(json!(serial))
        }
        "YEAR" => Ok(json!(date(&a[0])?.year())),
        "MONTH" => Ok(json!(date(&a[0])?.month())),
        "DAY" => Ok(json!(date(&a[0])?.day())),
        "INDEX" => {
            let v = a[0].as_array().ok_or("#VALUE! expected array")?;
            v.get(index(&a[1], 1)? - 1)
                .cloned()
                .ok_or("#REF! index out of range".into())
        }
        "MATCH" => {
            let v = a[1].as_array().ok_or("#VALUE! expected array")?;
            let mode = if a.len() == 3 { num(&a[2])? } else { 1.0 };
            if ![-1.0, 0.0, 1.0].contains(&mode) {
                return Err("#VALUE! MATCH mode must be -1, 0 or 1".into());
            }
            let mut found = None;
            for (i, v) in v.iter().enumerate() {
                let equal = if mode == 0.0
                    && a[0].is_string()
                    && text_value(&a[0]).contains(['*', '?', '~'])
                {
                    wildcard(
                        &text_value(v).to_lowercase(),
                        &text_value(&a[0]).to_lowercase(),
                        false,
                    )?
                } else {
                    compare(v, &a[0], "=")?
                };
                if equal && mode == 0.0 {
                    return Ok(json!(i + 1));
                }
                if equal {
                    found = Some(i + 1);
                }
                if mode != 0.0 && compare(v, &a[0], if mode == 1.0 { "<=" } else { ">=" })? {
                    found = Some(i + 1);
                }
            }
            found.map(|i| json!(i)).ok_or("#N/A".into())
        }
        "SUMIF" | "COUNTIF" | "AVERAGEIF" => {
            let range = a[0].as_array().ok_or("#VALUE! expected array")?;
            let sums = if a.len() == 3 {
                a[2].as_array().ok_or("#VALUE! expected array")?
            } else {
                range
            };
            if sums.len() != range.len() {
                return Err("#VALUE! arrays must have equal length".into());
            }
            let mut count = 0;
            let mut sum = 0.0;
            let mut numeric = 0;
            for (i, v) in range.iter().enumerate() {
                if criterion(v, &a[1])? {
                    count += 1;
                    if let Some(v) = sums[i].as_f64() {
                        sum += v;
                        numeric += 1;
                    }
                }
            }
            match n {
                "COUNTIF" => Ok(json!(count)),
                "SUMIF" => number(sum),
                _ => {
                    if numeric == 0 {
                        Err("#DIV/0!".into())
                    } else {
                        number(sum / numeric as f64)
                    }
                }
            }
        }
        _ => Err(format!("unsupported function {n}")),
    }
}
// Excel wildcards: * = any sequence, ? = one character, ~ = escape.
fn wildcard(text: &str, pattern: &str, prefix: bool) -> Result<bool, String> {
    wildcard_chars(
        &text.chars().collect::<Vec<_>>(),
        pattern,
        prefix,
        &mut 1_000_000,
    )
}
fn wildcard_chars(
    text: &[char],
    pattern: &str,
    prefix: bool,
    budget: &mut usize,
) -> Result<bool, String> {
    #[derive(Clone, Copy)]
    enum Token {
        Star,
        Any,
        Literal(char),
    }
    let mut tokens = Vec::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        tokens.push(match c {
            '*' => Token::Star,
            '?' => Token::Any,
            '~' => Token::Literal(chars.next().unwrap_or('~')),
            _ => Token::Literal(c),
        });
    }
    if tokens.len() > 256 {
        return Err("#VALUE! wildcard pattern exceeds 256 characters".into());
    }
    let (mut i, mut j) = (0, 0);
    let mut star = None;
    let mut retry = 0;
    while i < text.len() {
        if *budget == 0 {
            return Err("#VALUE! wildcard operation limit exceeded".into());
        }
        *budget -= 1;
        if prefix && j == tokens.len() {
            return Ok(true);
        }
        match tokens.get(j) {
            Some(Token::Literal(c)) if *c == text[i] => {
                i += 1;
                j += 1;
            }
            Some(Token::Any) => {
                i += 1;
                j += 1;
            }
            Some(Token::Star) => {
                star = Some(j);
                j += 1;
                retry = i;
            }
            _ => {
                if let Some(s) = star {
                    retry += 1;
                    i = retry;
                    j = s + 1;
                } else {
                    return Ok(false);
                }
            }
        }
    }
    while matches!(tokens.get(j), Some(Token::Star)) {
        j += 1;
    }
    Ok(j == tokens.len())
}

fn join_text(parts: impl Iterator<Item = String>, delimiter: &str) -> Result<Value, String> {
    let mut out = String::new();
    let mut first = true;
    for part in parts {
        let extra = part.len() + if first { 0 } else { delimiter.len() };
        if out.len().saturating_add(extra) > 1_000_000 {
            return Err("#VALUE! result exceeds 1 MB".into());
        }
        if !first {
            out.push_str(delimiter);
        }
        first = false;
        out.push_str(&part);
    }
    bounded_text(out)
}

fn bounded_text(s: String) -> Result<Value, String> {
    if s.len() > 1_000_000 {
        Err("#VALUE! result exceeds 1 MB".into())
    } else {
        Ok(json!(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(formula: &str) -> Result<Value, String> {
        let expr = parse(formula)?;
        let ctx = Ctx {
            all: &HashMap::new(),
            body: &json!({"nums":[1,2,3,4],"blanks":[null,"",2],"words":["a","b","a"],"words2":["a","a","b","A"]}),
            lookups: &Lookups::default(),
        };
        eval(&expr, &ctx, &mut HashMap::new())
    }
    #[test]
    fn every_catalog_function_has_a_behavior_test() {
        let cases = [
            ("SUM([nums])", json!(10)),
            ("AVERAGE([nums])", json!(2.5)),
            ("MIN([nums])", json!(1)),
            ("MAX([nums])", json!(4)),
            ("COUNT([blanks])", json!(1)),
            ("COUNTA([blanks])", json!(2)),
            ("COUNTBLANK([blanks])", json!(2)),
            ("PRODUCT([nums])", json!(24)),
            ("MEDIAN([nums])", json!(2.5)),
            ("ABS(-4)", json!(4)),
            ("ROUND(-1.25,1)", json!(-1.3)),
            ("ROUNDUP(-1.21,1)", json!(-1.3)),
            ("ROUNDDOWN(-1.29,1)", json!(-1.2)),
            ("INT(-1.2)", json!(-2)),
            ("MOD(-3,2)", json!(1)),
            ("POWER(2,3)", json!(8)),
            ("SQRT(9)", json!(3)),
            ("CEILING(3.1,2)", json!(4)),
            ("FLOOR(3.1,2)", json!(2)),
            ("IF(TRUE,7,1/0)", json!(7)),
            ("IFERROR(1/0,9)", json!(9)),
            ("AND(TRUE,1)", json!(true)),
            ("OR(FALSE,1)", json!(true)),
            ("NOT(0)", json!(true)),
            ("CONCAT(\"a\",\"b\")", json!("ab")),
            ("CONCATENATE(\"a\",\"b\")", json!("ab")),
            ("TEXTJOIN(\",\",TRUE,[blanks])", json!("2")),
            ("LEFT(\"føø\",2)", json!("fø")),
            ("RIGHT(\"føø\",2)", json!("øø")),
            ("MID(\"hello\",2,3)", json!("ell")),
            ("LEN(\"føø\")", json!(3)),
            ("LOWER(\"HELLO\")", json!("hello")),
            ("UPPER(\"hello\")", json!("HELLO")),
            ("PROPER(\"hello WORLD\")", json!("Hello World")),
            ("TRIM(\"  a  b  \")", json!("a b")),
            ("SUBSTITUTE(\"a-a-a\",\"a\",\"b\",2)", json!("a-b-a")),
            ("REPLACE(\"hello\",2,3,\"i\")", json!("hio")),
            ("FIND(\"ø\",\"føø\",3)", json!(3)),
            ("SEARCH(\"LO\",\"hello\")", json!(4)),
            ("EXACT(\"A\",\"a\")", json!(false)),
            ("VALUE(\"2.5\")", json!(2.5)),
            ("DATE(2024,2,29)", json!(45351)),
            ("YEAR(DATE(2024,2,29))", json!(2024)),
            ("MONTH(\"2024-02-29\")", json!(2)),
            ("DAY(\"2024-02-29\")", json!(29)),
            ("SUMIF([nums],\">2\")", json!(7)),
            ("COUNTIF([words],\"a\")", json!(2)),
            ("AVERAGEIF([nums],\">2\")", json!(3.5)),
            ("INDEX([nums],2)", json!(2)),
            ("MATCH(3,[nums],0)", json!(3)),
            ("SUMIFS([nums],[words2],\"a\",[nums],\">1\")", json!(6)),
            ("COUNTIFS([words2],\"a\")", json!(3)),
            ("AVERAGEIFS([nums],[words2],\"a\")", json!(7.0 / 3.0)),
            ("MINIFS([nums],[words2],\"a\")", json!(1)),
            ("MAXIFS([nums],[words2],\"b\")", json!(3)),
            ("XLOOKUP(\"b\",[words2],[nums])", json!(3)),
        ];
        assert_eq!(cases.len(), 56);
        assert_eq!(FUNCTIONS.len(), 56);
        for (name, _) in FUNCTIONS {
            assert!(
                cases
                    .iter()
                    .any(|(f, _)| f.starts_with(&format!("{name}("))),
                "Missing {name}"
            );
        }
        for (formula, expected) in cases {
            let actual = run(formula).unwrap_or_else(|e| panic!("{formula}: {e}"));
            if expected.is_number() {
                assert_eq!(actual.as_f64(), expected.as_f64(), "{formula}");
            } else {
                assert_eq!(actual, expected, "{formula}");
            }
        }
    }
    #[test]
    fn rejects_invalid_syntax_arity_and_limits() {
        for s in [
            "SUM()",
            "IF(1,2)",
            "UNKNOWN(1)",
            "1 +",
            "[broken",
            "\"broken",
            "1;evil",
            "1e9999",
            "SUMIFS([nums],[words])",
            "COUNTIFS([words])",
        ] {
            assert!(parse(s).is_err(), "{s}");
        }
        assert!(parse(&"(".repeat(65)).is_err());
        assert!(parse(&"1".repeat(4097)).is_err());
        assert!(run("1/0").is_err());
        assert!(run("SQRT(-1)").is_err());
        assert!(run("DATE(2024,1,-1e300)").is_err());
        assert!(
            function(
                "SUBSTITUTE",
                &[json!("a".repeat(2000)), json!("a"), json!("b".repeat(2000))]
            )
            .is_err()
        );
        assert!(
            function(
                "TEXTJOIN",
                &[
                    json!("x".repeat(600000)),
                    json!(true),
                    json!(["a", "b", "c"])
                ]
            )
            .is_err()
        );
        assert!(run("LEFT(\"a\",-1)").is_err());
        assert!(run("INDEX([nums],0)").is_err());
        assert!(run("IFERROR(SQRT(-1),3)").is_ok());
        assert_eq!(run("2+3*4=14").unwrap(), json!(true));
        assert_eq!(run("COUNTIF([words], \"?\")").unwrap(), json!(3));
        assert_eq!(run("SEARCH(\"h*o\", \"hello\")").unwrap(), json!(1));
        assert_eq!(run("MATCH(\"b*\", [words], 0)").unwrap(), json!(2));
        assert_eq!(run("\"a\"\"b\" & UPPER(\"c\")").unwrap(), json!("a\"bC"));
    }
    fn kindergartens() -> (Blueprint, Lookups) {
        let bp: Blueprint = serde_json::from_value(json!({"fields":[
            {"name":"org_nr","type":"string"},
            {"name":"ar","type":"number"},
            {"name":"weighted","type":"calculated","formula":
                "=IFERROR(SUMIFS([basil-arsmelding.arsverk];[basil-arsmelding.org_nr];[org_nr];[basil-arsmelding.ar];[ar]-2);\"\")*7/12+IFERROR(SUMIFS([basil-arsmelding.arsverk];[basil-arsmelding.org_nr];[org_nr];[basil-arsmelding.ar];[ar]-1);\"\")*5/12"},
            {"name":"kind","type":"calculated","formula":"=XLOOKUP([org_nr],[basil-arsmelding.org_nr],[basil-arsmelding.type],\"unknown\")"},
            {"name":"reports","type":"calculated","formula":"=COUNTIFS([basil-arsmelding.org_nr],[org_nr])"}
        ]}))
        .unwrap();
        let rows = [
            (2022, "920143067", 3.0, "Ordinær"),
            (2023, "920143067", 3.55, "Ordinær"),
            (2023, "920062407", 27.55, "Familie"),
            // Numbers stored as text still match numeric criteria.
            (2023, "828227572", 9.33, "Ordinær"),
        ];
        let mut lookups = Lookups::default();
        let column = |values: Vec<Value>| Arc::new(LookupColumn::new("basil-arsmelding", values));
        lookups.insert(
            "basil-arsmelding",
            "ar",
            column(rows.iter().map(|r| json!(r.0)).collect()),
        );
        lookups.insert(
            "basil-arsmelding",
            "org_nr",
            column(rows.iter().map(|r| json!(r.1)).collect()),
        );
        lookups.insert(
            "basil-arsmelding",
            "arsverk",
            column(rows.iter().map(|r| json!(r.2)).collect()),
        );
        lookups.insert(
            "basil-arsmelding",
            "type",
            column(rows.iter().map(|r| json!(r.3)).collect()),
        );
        (bp, lookups)
    }
    #[test]
    fn formulas_aggregate_columns_of_other_collections() {
        let (bp, lookups) = kindergartens();
        crate::validate_blueprint(&bp).unwrap();
        assert_eq!(
            lookup_references(&bp),
            ["arsverk", "org_nr", "ar", "type"]
                .iter()
                .map(|f| ("basil-arsmelding".to_string(), f.to_string()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
        let (body, errors) =
            calculate_document_with(&bp, &json!({"org_nr":"920143067","ar":2024}), &lookups);
        assert!(errors.is_empty(), "{errors:?}");
        let expected = 3.0 * 7.0 / 12.0 + 3.55 * 5.0 / 12.0;
        assert!((body["weighted"].as_f64().unwrap() - expected).abs() < 1e-9);
        assert_eq!(body["kind"], json!("Ordinær"));
        assert_eq!(body["reports"], json!(2));
        // An org number given as a number matches text values.
        let (body, _) =
            calculate_document_with(&bp, &json!({"org_nr":828227572,"ar":2025}), &lookups);
        assert!((body["weighted"].as_f64().unwrap() - 9.33 * 7.0 / 12.0).abs() < 1e-9);
        let (body, _) = calculate_document_with(&bp, &json!({"org_nr":"1","ar":2025}), &lookups);
        assert_eq!(body["weighted"].as_f64(), Some(0.0));
        assert_eq!(body["kind"], json!("unknown"));
        // Without the other collection's data the fields report why.
        let (body, errors) = calculate_document(&bp, &json!({"org_nr":"1","ar":2025}));
        assert_eq!(body["reports"], Value::Null);
        assert!(errors["reports"].contains("#REF!"), "{errors:?}");
    }
    #[test]
    fn indexed_and_scanned_criteria_agree() {
        let (_, lookups) = kindergartens();
        let all = HashMap::new();
        let ctx = |body| Ctx {
            all: &all,
            body,
            lookups: &lookups,
        };
        let body = json!({"sums":[3.0,3.55,27.55,9.33],"list":[2022,2023,2023,2023],"orgs":["920143067","920143067","920062407","828227572"]});
        for (indexed, scanned) in [
            (
                "SUMIFS([basil-arsmelding.arsverk],[basil-arsmelding.ar],2023)",
                "SUMIFS([sums],[list],2023)",
            ),
            (
                "COUNTIFS([basil-arsmelding.org_nr],\"=920143067\")",
                "COUNTIF([orgs],\"=920143067\")",
            ),
            (
                "COUNTIFS([basil-arsmelding.ar],\">2022\")",
                "COUNTIF([list],\">2022\")",
            ),
            ("COUNTIFS([basil-arsmelding.type],\"ORDINÆR\")", "3"),
        ] {
            let a = eval(&parse(indexed).unwrap(), &ctx(&body), &mut HashMap::new()).unwrap();
            let b = eval(&parse(scanned).unwrap(), &ctx(&body), &mut HashMap::new()).unwrap();
            assert_eq!(a.as_f64(), b.as_f64(), "{indexed}");
        }
        let mismatched = parse("SUMIFS([basil-arsmelding.arsverk],[orgs],1,[list],2)").unwrap();
        assert!(eval(&mismatched, &ctx(&body), &mut HashMap::new()).is_err());
    }
    #[test]
    fn cross_references_rename_and_validate() {
        let renamed = rename_formula_references("=[a.b] & \"[a.b]\" & [c]", |n| {
            n.strip_prefix("a.").map(|f| format!("z.{f}"))
        });
        assert_eq!(renamed, "=[z.b] & \"[a.b]\" & [c]");
        let bp: Blueprint = serde_json::from_value(json!({"fields":[
            {"name":"x","type":"calculated","formula":"=SUM([Bad Name.field])"}
        ]}))
        .unwrap();
        assert!(crate::validate_blueprint(&bp).is_err());
    }
    #[test]
    fn dependencies_recalculate_and_bad_schemas_are_rejected() {
        let bp: Blueprint = serde_json::from_value(json!({"fields":[
            {"name":"net","type":"calculated","formula":"=[total] * 0.8"},
            {"name":"price","type":"number"},
            {"name":"quantity","type":"number"},
            {"name":"total","type":"calculated","formula":"=[price] * [quantity]"}
        ]}))
        .unwrap();
        crate::validate_blueprint(&bp).unwrap();
        let (body, errors) = calculate_document(&bp, &json!({"price":10,"quantity":2,"total":999}));
        assert!(errors.is_empty());
        assert_eq!(body["net"].as_f64(), Some(16.0));
        let (body, _) = calculate_document(&bp, &json!({"price":20,"quantity":2}));
        assert_eq!(body["net"].as_f64(), Some(32.0));
        let mut bad = bp.clone();
        bad.fields[3].formula = Some("=[net]".into());
        assert!(
            crate::validate_blueprint(&bad)
                .unwrap_err()
                .contains("circular")
        );
        bad.fields[3].formula = Some("=[missing]".into());
        assert!(
            crate::validate_blueprint(&bad)
                .unwrap_err()
                .contains("unknown")
        );
        let source = source_document(&bp, &body);
        assert!(source.get("net").is_none());
        assert!(source.get("total").is_none());
    }
}

/// Rewrites `[old]` field references to `[new]`, all at once, leaving string
/// literals and other text untouched. Unparseable formulas are rewritten the
/// same way, so a later fix still sees the new names.
pub fn rename_formula_fields(
    formula: &str,
    renames: &std::collections::BTreeMap<String, String>,
) -> String {
    rename_formula_references(formula, |name| renames.get(name).cloned())
}

/// Rewrites each `[name]` that `rename` maps to a new name, leaving string
/// literals untouched. Used for renamed fields and for `[collection.field]`
/// references when another collection or its fields are renamed.
pub fn rename_formula_references(formula: &str, rename: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(formula.len());
    let mut chars = formula.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    out.push(chars.next().unwrap());
                } else {
                    in_string = false;
                }
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == '[' {
            let mut name = String::new();
            let mut closed = false;
            for next in chars.by_ref() {
                if next == ']' {
                    closed = true;
                    break;
                }
                name.push(next);
            }
            out.push('[');
            out.push_str(&rename(&name).unwrap_or(name));
            if closed {
                out.push(']');
            }
        } else {
            out.push(c);
        }
    }
    out
}
