//! Vulse SQL: `SELECT`, `UPDATE` and `DELETE` statements over a collection's
//! records. Statements are parsed and evaluated here, against records as
//! JSON objects; the server applies changes through the normal record path.
//!
//! Records have their fields plus `id`, `created_at` and `updated_at`.
//! Comparisons treat numbers and numeric text as numbers; `=` on text is
//! case-sensitive and `LIKE` is not. Functions are the formula functions,
//! plus `COUNT`, `SUM`, `AVG`, `MIN` and `MAX` over groups, `COALESCE`,
//! `IFNULL`, `LENGTH` and `SUBSTR`.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Rows a `SELECT` returns; `total` still counts all of them.
pub const MAX_SQL_ROWS: usize = 1000;
/// Records one `UPDATE` or `DELETE` may change.
pub const MAX_SQL_CHANGES: usize = 50_000;
/// Changed records shown in a preview.
pub const MAX_SQL_SAMPLE: usize = 20;
/// Built-in record properties, readable but not writable.
pub const SQL_RECORD_COLUMNS: [&str; 3] = ["id", "created_at", "updated_at"];

pub const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "GROUP", "BY", "ORDER", "ASC", "DESC", "LIMIT", "OFFSET", "AS",
    "UPDATE", "SET", "DELETE", "AND", "OR", "NOT", "IS", "NULL", "IN", "BETWEEN", "LIKE", "TRUE",
    "FALSE",
];
pub const SQL_AGGREGATES: [&str; 5] = ["COUNT", "SUM", "AVG", "MIN", "MAX"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlRequest {
    pub sql: String,
    /// The confirmation from a preview, to apply exactly those changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SqlChange {
    pub id: String,
    pub title: String,
    /// The changed fields' current values (all fields for a delete).
    pub before: Map<String, Value>,
    /// The new values; `None` when the record is deleted.
    pub after: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SqlResult {
    /// `select`, `update` or `delete`.
    pub kind: String,
    pub collection_id: String,
    pub collection: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// Matching rows (or groups) before `LIMIT`.
    pub total: u64,
    pub truncated: bool,
    /// Records the statement changes.
    pub affected: u64,
    pub changes: Vec<SqlChange>,
    /// Records that would become invalid; the statement cannot run.
    pub problems: Vec<String>,
    /// Send back as `confirm` to apply a previewed change.
    pub confirmation: Option<String>,
    /// Whether the changes were applied.
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SqlError {
    pub message: String,
    /// Character offset of the problem, when known.
    pub at: Option<usize>,
}
impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            Some(at) => write!(f, "{} (at character {})", self.message, at + 1),
            None => f.write_str(&self.message),
        }
    }
}
fn error<T>(message: impl Into<String>, at: usize) -> Result<T, SqlError> {
    Err(SqlError {
        message: message.into(),
        at: Some(at),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum SqlExpr {
    Field(String),
    Literal(Value),
    Negate(Box<SqlExpr>),
    Not(Box<SqlExpr>),
    Binary(String, Box<SqlExpr>, Box<SqlExpr>),
    IsNull(Box<SqlExpr>, bool),
    In(Box<SqlExpr>, Vec<SqlExpr>, bool),
    Between(Box<SqlExpr>, Box<SqlExpr>, Box<SqlExpr>, bool),
    Like(Box<SqlExpr>, Box<SqlExpr>, bool),
    /// A function; `COUNT(*)` has no arguments and `star` set.
    Call(String, Vec<SqlExpr>, bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SqlColumn {
    pub expr: SqlExpr,
    pub label: String,
    /// Whether `label` came from `AS`.
    pub alias: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Select {
    /// Empty for `SELECT *`.
    pub columns: Vec<SqlColumn>,
    pub from: String,
    pub filter: Option<SqlExpr>,
    pub group_by: Vec<SqlExpr>,
    pub order_by: Vec<(SqlExpr, bool)>,
    pub limit: Option<usize>,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Select(Select),
    Update {
        table: String,
        set: Vec<(String, SqlExpr)>,
        filter: Option<SqlExpr>,
    },
    Delete {
        table: String,
        filter: Option<SqlExpr>,
    },
}

impl Statement {
    pub fn table(&self) -> &str {
        match self {
            Statement::Select(s) => &s.from,
            Statement::Update { table, .. } | Statement::Delete { table, .. } => table,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Statement::Select(_) => "select",
            Statement::Update { .. } => "update",
            Statement::Delete { .. } => "delete",
        }
    }
    pub fn filter(&self) -> Option<&SqlExpr> {
        match self {
            Statement::Select(s) => s.filter.as_ref(),
            Statement::Update { filter, .. } | Statement::Delete { filter, .. } => filter.as_ref(),
        }
    }
    /// Field names the statement reads or writes, except `ORDER BY`
    /// references to column aliases.
    pub fn fields(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut add = |e: &SqlExpr| collect_fields(e, &mut out);
        match self {
            Statement::Select(s) => {
                s.columns.iter().for_each(|c| add(&c.expr));
                s.filter.iter().for_each(&mut add);
                s.group_by.iter().for_each(&mut add);
                for (e, _) in &s.order_by {
                    let alias = matches!(e, SqlExpr::Field(n) if s.columns.iter().any(|c| c.alias && c.label == *n));
                    if !alias {
                        add(e);
                    }
                }
            }
            Statement::Update { set, filter, .. } => {
                for (field, e) in set {
                    add(&SqlExpr::Field(field.clone()));
                    add(e);
                }
                filter.iter().for_each(&mut add);
            }
            Statement::Delete { filter, .. } => filter.iter().for_each(add),
        }
        out.sort();
        out.dedup();
        out
    }
}

fn collect_fields(e: &SqlExpr, out: &mut Vec<String>) {
    match e {
        SqlExpr::Field(n) => out.push(n.clone()),
        SqlExpr::Literal(_) => {}
        SqlExpr::Negate(a) | SqlExpr::Not(a) | SqlExpr::IsNull(a, _) => collect_fields(a, out),
        SqlExpr::Binary(_, a, b) | SqlExpr::Like(a, b, _) => {
            collect_fields(a, out);
            collect_fields(b, out);
        }
        SqlExpr::Between(a, b, c, _) => {
            collect_fields(a, out);
            collect_fields(b, out);
            collect_fields(c, out);
        }
        SqlExpr::In(a, list, _) => {
            collect_fields(a, out);
            list.iter().for_each(|e| collect_fields(e, out));
        }
        SqlExpr::Call(_, args, _) => args.iter().for_each(|e| collect_fields(e, out)),
    }
}

// ---------------------------------------------------------------- lexer

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// A bare word: keyword, field, collection or function name.
    Word(String),
    /// A `"quoted"` or `` `quoted` `` name.
    Quoted(String),
    Text(String),
    Number(f64),
    Symbol(&'static str),
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}

const SYMBOLS: [&str; 19] = [
    "<=", ">=", "<>", "!=", "==", "||", "=", "<", ">", "+", "-", "*", "/", "%", "(", ")", ",", ";",
    ".",
];

fn lex(sql: &str) -> Result<Vec<Token>, SqlError> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        let tok = if c == '\'' || c == '"' || c == '`' {
            let mut text = String::new();
            i += 1;
            loop {
                let Some(&next) = chars.get(i) else {
                    return error("this quote is never closed", start);
                };
                i += 1;
                if next == c {
                    if chars.get(i) == Some(&c) {
                        text.push(c);
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    text.push(next);
                }
            }
            if c == '\'' {
                Tok::Text(text)
            } else {
                Tok::Quoted(text)
            }
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            match text.parse::<f64>() {
                Ok(n) => Tok::Number(n),
                Err(_) => return error(format!("'{text}' is not a number"), start),
            }
        } else if c.is_alphabetic() || c == '_' {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            Tok::Word(chars[start..i].iter().collect())
        } else {
            let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
            let Some(symbol) = SYMBOLS.iter().find(|s| rest.starts_with(**s)) else {
                return error(format!("unexpected '{c}'"), i);
            };
            i += symbol.chars().count();
            Tok::Symbol(symbol)
        };
        out.push(Token { tok, start, end: i });
    }
    Ok(out)
}

// ---------------------------------------------------------------- parser

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    chars: &'a [char],
}

impl Parser<'_> {
    fn at(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map(|t| t.start)
            .unwrap_or(self.chars.len())
    }
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }
    fn is_keyword(&self, keyword: &str) -> bool {
        matches!(self.peek(), Some(Tok::Word(w)) if w.eq_ignore_ascii_case(keyword))
    }
    fn keyword(&mut self, keyword: &str) -> bool {
        let found = self.is_keyword(keyword);
        if found {
            self.pos += 1;
        }
        found
    }
    fn expect_keyword(&mut self, keyword: &str) -> Result<(), SqlError> {
        if self.keyword(keyword) {
            Ok(())
        } else {
            error(format!("expected {keyword}"), self.at())
        }
    }
    fn symbol(&mut self, symbol: &str) -> bool {
        let found = matches!(self.peek(), Some(Tok::Symbol(s)) if *s == symbol);
        if found {
            self.pos += 1;
        }
        found
    }
    fn expect_symbol(&mut self, symbol: &str) -> Result<(), SqlError> {
        if self.symbol(symbol) {
            Ok(())
        } else {
            error(format!("expected '{symbol}'"), self.at())
        }
    }
    /// A field name: a bare word that is not a keyword, or a quoted name.
    fn name(&mut self, what: &str) -> Result<String, SqlError> {
        match self.peek().cloned() {
            Some(Tok::Word(w)) if !SQL_KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(&w)) => {
                self.pos += 1;
                Ok(w)
            }
            Some(Tok::Quoted(w)) => {
                self.pos += 1;
                Ok(w)
            }
            Some(Tok::Word(w)) => error(
                format!(
                    "{} is a keyword; write \"{}\" to use it as a {what}",
                    w.to_ascii_uppercase(),
                    w.to_lowercase()
                ),
                self.at(),
            ),
            _ => error(format!("expected a {what}"), self.at()),
        }
    }
    /// A collection name, which may contain hyphens: `pensjon-2024`.
    fn table(&mut self) -> Result<String, SqlError> {
        let mut name = self.name("collection name")?;
        while let (Some(dash), Some(next)) =
            (self.tokens.get(self.pos), self.tokens.get(self.pos + 1))
        {
            let joined = self.tokens[self.pos - 1].end == dash.start && dash.end == next.start;
            let part = match &next.tok {
                Tok::Word(w) => w.clone(),
                Tok::Number(_) => self.chars[next.start..next.end].iter().collect(),
                _ => break,
            };
            if dash.tok != Tok::Symbol("-") || !joined {
                break;
            }
            name.push('-');
            name.push_str(&part);
            self.pos += 2;
        }
        Ok(name.to_ascii_lowercase())
    }
    fn usize(&mut self, what: &str) -> Result<usize, SqlError> {
        match self.peek() {
            Some(Tok::Number(n)) if n.fract() == 0.0 && *n >= 0.0 => {
                let n = *n as usize;
                self.pos += 1;
                Ok(n)
            }
            _ => error(format!("{what} needs a whole number"), self.at()),
        }
    }

    fn statement(&mut self) -> Result<Statement, SqlError> {
        let statement = if self.keyword("SELECT") {
            Statement::Select(self.select()?)
        } else if self.keyword("UPDATE") {
            let table = self.table()?;
            self.expect_keyword("SET")?;
            let mut set = Vec::new();
            loop {
                let field = self.name("field name")?;
                if !self.symbol("=") {
                    return error("expected '=' after the field name", self.at());
                }
                set.push((field, self.expr()?));
                if !self.symbol(",") {
                    break;
                }
            }
            let filter = self.filter()?;
            Statement::Update { table, set, filter }
        } else if self.keyword("DELETE") {
            self.expect_keyword("FROM")?;
            let table = self.table()?;
            let filter = self.filter()?;
            Statement::Delete { table, filter }
        } else {
            return error("start with SELECT, UPDATE or DELETE", self.at());
        };
        self.symbol(";");
        if self.pos < self.tokens.len() {
            return error("unexpected text after the statement", self.at());
        }
        Ok(statement)
    }
    fn filter(&mut self) -> Result<Option<SqlExpr>, SqlError> {
        Ok(if self.keyword("WHERE") {
            Some(self.expr()?)
        } else {
            None
        })
    }
    fn select(&mut self) -> Result<Select, SqlError> {
        let mut columns = Vec::new();
        if !self.symbol("*") {
            loop {
                let start = self.at();
                let expr = self.expr()?;
                let end = self.tokens[self.pos - 1].end;
                let (label, alias) = if self.keyword("AS") {
                    (self.name("column name")?, true)
                } else {
                    (self.chars[start..end].iter().collect(), false)
                };
                columns.push(SqlColumn { expr, label, alias });
                if !self.symbol(",") {
                    break;
                }
            }
        }
        self.expect_keyword("FROM")?;
        let from = self.table()?;
        let filter = self.filter()?;
        let mut group_by = Vec::new();
        if self.keyword("GROUP") {
            self.expect_keyword("BY")?;
            loop {
                group_by.push(self.expr()?);
                if !self.symbol(",") {
                    break;
                }
            }
        }
        let mut order_by = Vec::new();
        if self.keyword("ORDER") {
            self.expect_keyword("BY")?;
            loop {
                let expr = self.expr()?;
                let descending = if self.keyword("DESC") {
                    true
                } else {
                    self.keyword("ASC");
                    false
                };
                order_by.push((expr, descending));
                if !self.symbol(",") {
                    break;
                }
            }
        }
        let limit = if self.keyword("LIMIT") {
            Some(self.usize("LIMIT")?)
        } else {
            None
        };
        let offset = if self.keyword("OFFSET") {
            self.usize("OFFSET")?
        } else {
            0
        };
        Ok(Select {
            columns,
            from,
            filter,
            group_by,
            order_by,
            limit,
            offset,
        })
    }

    fn expr(&mut self) -> Result<SqlExpr, SqlError> {
        self.or()
    }
    fn or(&mut self) -> Result<SqlExpr, SqlError> {
        let mut left = self.and()?;
        while self.keyword("OR") {
            left = SqlExpr::Binary("OR".into(), Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }
    fn and(&mut self) -> Result<SqlExpr, SqlError> {
        let mut left = self.not()?;
        while self.keyword("AND") {
            left = SqlExpr::Binary("AND".into(), Box::new(left), Box::new(self.not()?));
        }
        Ok(left)
    }
    fn not(&mut self) -> Result<SqlExpr, SqlError> {
        if self.keyword("NOT") {
            return Ok(SqlExpr::Not(Box::new(self.not()?)));
        }
        self.comparison()
    }
    fn comparison(&mut self) -> Result<SqlExpr, SqlError> {
        let left = self.concat()?;
        for op in ["=", "==", "<>", "!=", "<=", ">=", "<", ">"] {
            if self.symbol(op) {
                let op = match op {
                    "==" => "=",
                    "!=" => "<>",
                    op => op,
                };
                return Ok(SqlExpr::Binary(
                    op.into(),
                    Box::new(left),
                    Box::new(self.concat()?),
                ));
            }
        }
        if self.keyword("IS") {
            let not = self.keyword("NOT");
            self.expect_keyword("NULL")?;
            return Ok(SqlExpr::IsNull(Box::new(left), not));
        }
        let not = self.keyword("NOT");
        if self.keyword("IN") {
            self.expect_symbol("(")?;
            let mut list = vec![self.expr()?];
            while self.symbol(",") {
                list.push(self.expr()?);
            }
            self.expect_symbol(")")?;
            return Ok(SqlExpr::In(Box::new(left), list, not));
        }
        if self.keyword("BETWEEN") {
            let low = self.concat()?;
            self.expect_keyword("AND")?;
            let high = self.concat()?;
            return Ok(SqlExpr::Between(
                Box::new(left),
                Box::new(low),
                Box::new(high),
                not,
            ));
        }
        if self.keyword("LIKE") {
            return Ok(SqlExpr::Like(Box::new(left), Box::new(self.concat()?), not));
        }
        if not {
            return error("expected IN, BETWEEN or LIKE after NOT", self.at());
        }
        Ok(left)
    }
    fn concat(&mut self) -> Result<SqlExpr, SqlError> {
        let mut left = self.additive()?;
        while self.symbol("||") {
            left = SqlExpr::Binary("||".into(), Box::new(left), Box::new(self.additive()?));
        }
        Ok(left)
    }
    fn additive(&mut self) -> Result<SqlExpr, SqlError> {
        let mut left = self.multiplicative()?;
        loop {
            let op = if self.symbol("+") {
                "+"
            } else if self.symbol("-") {
                "-"
            } else {
                return Ok(left);
            };
            left = SqlExpr::Binary(op.into(), Box::new(left), Box::new(self.multiplicative()?));
        }
    }
    fn multiplicative(&mut self) -> Result<SqlExpr, SqlError> {
        let mut left = self.unary()?;
        loop {
            let op = if self.symbol("*") {
                "*"
            } else if self.symbol("/") {
                "/"
            } else if self.symbol("%") {
                "%"
            } else {
                return Ok(left);
            };
            left = SqlExpr::Binary(op.into(), Box::new(left), Box::new(self.unary()?));
        }
    }
    fn unary(&mut self) -> Result<SqlExpr, SqlError> {
        if self.symbol("-") {
            return Ok(SqlExpr::Negate(Box::new(self.unary()?)));
        }
        if self.symbol("+") {
            return self.unary();
        }
        self.primary()
    }
    fn primary(&mut self) -> Result<SqlExpr, SqlError> {
        let at = self.at();
        match self.peek().cloned() {
            Some(Tok::Number(n)) => {
                self.pos += 1;
                Ok(SqlExpr::Literal(number(n)))
            }
            Some(Tok::Text(t)) => {
                self.pos += 1;
                Ok(SqlExpr::Literal(Value::String(t)))
            }
            Some(Tok::Symbol("(")) => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect_symbol(")")?;
                Ok(e)
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("NULL") => {
                self.pos += 1;
                Ok(SqlExpr::Literal(Value::Null))
            }
            Some(Tok::Word(w))
                if w.eq_ignore_ascii_case("TRUE") || w.eq_ignore_ascii_case("FALSE") =>
            {
                self.pos += 1;
                Ok(SqlExpr::Literal(json!(w.eq_ignore_ascii_case("TRUE"))))
            }
            Some(Tok::Word(w))
                if matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.tok),
                    Some(Tok::Symbol("("))
                ) =>
            {
                self.pos += 2;
                let name = w.to_ascii_uppercase();
                if self.symbol("*") {
                    self.expect_symbol(")")?;
                    if name != "COUNT" {
                        return error("only COUNT(*) takes *", at);
                    }
                    return Ok(SqlExpr::Call(name, Vec::new(), true));
                }
                let mut args = Vec::new();
                if !self.symbol(")") {
                    loop {
                        args.push(self.expr()?);
                        if self.symbol(")") {
                            break;
                        }
                        self.expect_symbol(",")?;
                    }
                }
                check_function(&name, args.len()).map_err(|message| SqlError {
                    message,
                    at: Some(at),
                })?;
                Ok(SqlExpr::Call(name, args, false))
            }
            Some(Tok::Word(_)) | Some(Tok::Quoted(_)) => {
                Ok(SqlExpr::Field(self.name("field name")?))
            }
            None => error("the statement ends too early", at),
            Some(_) => error("expected a value, field or function", at),
        }
    }
}

fn check_function(name: &str, args: usize) -> Result<(), String> {
    match name {
        "COUNT" | "SUM" | "AVG" if args != 1 => Err(format!("{name} takes one value")),
        "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" | "COALESCE" | "IFNULL" => Ok(()),
        "LENGTH" if args != 1 => Err("LENGTH takes one value".into()),
        "SUBSTR" if !(2..=3).contains(&args) => {
            Err("SUBSTR takes text, a start and a length".into())
        }
        "LENGTH" | "SUBSTR" => Ok(()),
        _ if crate::FUNCTIONS.iter().any(|(n, _)| *n == name) => Ok(()),
        _ => Err(format!("unknown function {name}")),
    }
}

/// Parses one statement.
pub fn parse_sql(sql: &str) -> Result<Statement, SqlError> {
    if sql.len() > 20_000 {
        return Err(SqlError {
            message: "statements are limited to 20,000 characters".into(),
            at: None,
        });
    }
    let chars: Vec<char> = sql.chars().collect();
    let mut parser = Parser {
        tokens: lex(sql)?,
        pos: 0,
        chars: &chars,
    };
    parser.statement()
}

// ---------------------------------------------------------------- evaluation

/// A record: its fields plus `id`, `created_at` and `updated_at`.
pub type SqlRow = Map<String, Value>;

enum Scope<'a> {
    Row(&'a SqlRow),
    Group(&'a [&'a SqlRow]),
}

fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
        json!(n as i64)
    } else {
        serde_json::Number::from_f64(n)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}
fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}
fn text(v: &Value) -> String {
    crate::text_value(v)
}

/// Orders two values: numbers and numeric text by value, otherwise as text.
/// `None` when either is empty.
fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    if a.is_null() || b.is_null() {
        return None;
    }
    if let (Value::Bool(x), Value::Bool(y)) = (a, b) {
        return Some(x.cmp(y));
    }
    if let (Some(x), Some(y)) = (as_number(a), as_number(b)) {
        return x.partial_cmp(&y);
    }
    Some(text(a).cmp(&text(b)))
}
/// SQL truth: `None` is unknown.
fn truth(v: &Value) -> Result<Option<bool>, String> {
    match v {
        Value::Null => Ok(None),
        Value::Bool(b) => Ok(Some(*b)),
        _ => match as_number(v) {
            Some(n) => Ok(Some(n != 0.0)),
            None => Err(format!(
                "'{}' is not a condition; compare it, e.g. field = '{}'",
                text(v),
                text(v)
            )),
        },
    }
}
fn known(b: Option<bool>) -> Value {
    b.map(Value::Bool).unwrap_or(Value::Null)
}

fn like(value: &str, pattern: &str) -> bool {
    let value: Vec<char> = value.to_lowercase().chars().collect();
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let (mut v, mut p) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '_' || pattern[p] == value[v]) {
            v += 1;
            p += 1;
        } else if p < pattern.len() && pattern[p] == '%' {
            star = Some(p);
            mark = v;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            v = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '%' {
        p += 1;
    }
    p == pattern.len()
}

fn eval(e: &SqlExpr, scope: &Scope) -> Result<Value, String> {
    match e {
        SqlExpr::Field(name) => Ok(match scope {
            Scope::Row(row) => row.get(name).cloned().unwrap_or(Value::Null),
            Scope::Group(rows) => rows
                .first()
                .and_then(|r| r.get(name).cloned())
                .unwrap_or(Value::Null),
        }),
        SqlExpr::Literal(v) => Ok(v.clone()),
        SqlExpr::Negate(a) => {
            let v = eval(a, scope)?;
            if v.is_null() {
                return Ok(Value::Null);
            }
            as_number(&v)
                .map(|n| number(-n))
                .ok_or_else(|| format!("cannot negate '{}'", text(&v)))
        }
        SqlExpr::Not(a) => Ok(known(truth(&eval(a, scope)?)?.map(|b| !b))),
        SqlExpr::Binary(op, a, b) if op == "AND" || op == "OR" => {
            let a = truth(&eval(a, scope)?)?;
            // Short-circuit like SQL: FALSE AND x is FALSE, TRUE OR x is TRUE.
            if (op == "AND" && a == Some(false)) || (op == "OR" && a == Some(true)) {
                return Ok(known(a));
            }
            let b = truth(&eval(b, scope)?)?;
            Ok(known(match (op.as_str(), a, b) {
                ("AND", _, Some(false)) => Some(false),
                ("AND", Some(true), Some(true)) => Some(true),
                ("OR", _, Some(true)) => Some(true),
                ("OR", Some(false), Some(false)) => Some(false),
                _ => None,
            }))
        }
        SqlExpr::Binary(op, a, b) => {
            let (a, b) = (eval(a, scope)?, eval(b, scope)?);
            match op.as_str() {
                "=" | "<>" | "<" | ">" | "<=" | ">=" => {
                    Ok(known(compare(&a, &b).map(|o| match op.as_str() {
                        "=" => o == Ordering::Equal,
                        "<>" => o != Ordering::Equal,
                        "<" => o == Ordering::Less,
                        ">" => o == Ordering::Greater,
                        "<=" => o != Ordering::Greater,
                        _ => o != Ordering::Less,
                    })))
                }
                "||" if a.is_null() || b.is_null() => Ok(Value::Null),
                "||" => Ok(json!(text(&a) + &text(&b))),
                _ if a.is_null() || b.is_null() => Ok(Value::Null),
                _ => {
                    let x =
                        as_number(&a).ok_or_else(|| format!("'{}' is not a number", text(&a)))?;
                    let y =
                        as_number(&b).ok_or_else(|| format!("'{}' is not a number", text(&b)))?;
                    Ok(number(match op.as_str() {
                        "+" => x + y,
                        "-" => x - y,
                        "*" => x * y,
                        _ if y == 0.0 => return Err("division by zero".into()),
                        "/" => x / y,
                        _ => x % y,
                    }))
                }
            }
        }
        SqlExpr::IsNull(a, not) => {
            let v = eval(a, scope)?;
            let empty = v.is_null() || v == json!("");
            Ok(json!(empty != *not))
        }
        SqlExpr::In(a, list, not) => {
            let v = eval(a, scope)?;
            let mut unknown = v.is_null();
            for item in list {
                match compare(&v, &eval(item, scope)?) {
                    Some(Ordering::Equal) => return Ok(json!(!not)),
                    None => unknown = true,
                    _ => {}
                }
            }
            Ok(if unknown { Value::Null } else { json!(*not) })
        }
        SqlExpr::Between(a, low, high, not) => {
            let v = eval(a, scope)?;
            let inside = match (
                compare(&v, &eval(low, scope)?),
                compare(&v, &eval(high, scope)?),
            ) {
                (Some(l), Some(h)) => Some(l != Ordering::Less && h != Ordering::Greater),
                _ => None,
            };
            Ok(known(inside.map(|b| b != *not)))
        }
        SqlExpr::Like(a, pattern, not) => {
            let (v, p) = (eval(a, scope)?, eval(pattern, scope)?);
            if v.is_null() || p.is_null() {
                return Ok(Value::Null);
            }
            Ok(json!(like(&text(&v), &text(&p)) != *not))
        }
        SqlExpr::Call(name, args, star) => call(name, args, *star, scope),
    }
}

fn is_aggregate(name: &str, args: usize) -> bool {
    matches!(name, "COUNT" | "SUM" | "AVG") || (matches!(name, "MIN" | "MAX") && args == 1)
}

fn call(name: &str, args: &[SqlExpr], star: bool, scope: &Scope) -> Result<Value, String> {
    if is_aggregate(name, args.len()) {
        let Scope::Group(rows) = scope else {
            return Err(format!(
                "{name}(…) adds up many records; use it in SELECT, not in WHERE or SET"
            ));
        };
        if star {
            return Ok(json!(rows.len()));
        }
        let mut values = Vec::new();
        for row in rows.iter() {
            let v = eval(&args[0], &Scope::Row(row))?;
            if !v.is_null() && v != json!("") {
                values.push(v);
            }
        }
        return Ok(match name {
            "COUNT" => json!(values.len()),
            "SUM" | "AVG" => {
                let numbers: Vec<f64> = values.iter().filter_map(as_number).collect();
                let sum: f64 = numbers.iter().sum();
                if name == "SUM" {
                    number(sum)
                } else if numbers.is_empty() {
                    Value::Null
                } else {
                    number(sum / numbers.len() as f64)
                }
            }
            _ => values
                .into_iter()
                .reduce(|best, v| {
                    let better = compare(&v, &best).is_some_and(|o| {
                        if name == "MIN" {
                            o == Ordering::Less
                        } else {
                            o == Ordering::Greater
                        }
                    });
                    if better { v } else { best }
                })
                .unwrap_or(Value::Null),
        });
    }
    let values = args
        .iter()
        .map(|a| eval(a, scope))
        .collect::<Result<Vec<_>, _>>()?;
    match name {
        "COALESCE" | "IFNULL" => Ok(values
            .into_iter()
            .find(|v| !v.is_null())
            .unwrap_or(Value::Null)),
        "LENGTH" => crate::call_function("LEN", &values),
        "SUBSTR" => {
            let mut values = values;
            if values.len() == 2 {
                values.push(json!(1_000_000));
            }
            crate::call_function("MID", &values)
        }
        "MIN" | "MAX" => crate::call_function(name, &values),
        _ => crate::call_function(name, &values).map(|v| match v.as_f64() {
            Some(n) => number(n),
            None => v,
        }),
    }
}

/// A result row; whole numbers print without a decimal point.
fn output(columns: &[SqlColumn], scope: &Scope) -> Result<Vec<Value>, String> {
    columns
        .iter()
        .map(|c| {
            eval(&c.expr, scope).map(|v| match v.as_f64() {
                Some(n) if v.is_f64() => number(n),
                _ => v,
            })
        })
        .collect()
}

fn has_aggregate(e: &SqlExpr) -> bool {
    match e {
        SqlExpr::Call(name, args, _) => {
            is_aggregate(name, args.len()) || args.iter().any(has_aggregate)
        }
        SqlExpr::Field(_) | SqlExpr::Literal(_) => false,
        SqlExpr::Negate(a) | SqlExpr::Not(a) | SqlExpr::IsNull(a, _) => has_aggregate(a),
        SqlExpr::Binary(_, a, b) | SqlExpr::Like(a, b, _) => has_aggregate(a) || has_aggregate(b),
        SqlExpr::Between(a, b, c, _) => has_aggregate(a) || has_aggregate(b) || has_aggregate(c),
        SqlExpr::In(a, list, _) => has_aggregate(a) || list.iter().any(has_aggregate),
    }
}

/// Whether a record matches `WHERE`; unknown counts as no.
pub fn sql_matches(filter: Option<&SqlExpr>, row: &SqlRow) -> Result<bool, String> {
    match filter {
        None => Ok(true),
        Some(e) => Ok(truth(&eval(e, &Scope::Row(row))?)? == Some(true)),
    }
}

/// The new values `SET` gives a record, computed from its current values.
pub fn sql_assignments(
    set: &[(String, SqlExpr)],
    row: &SqlRow,
) -> Result<Map<String, Value>, String> {
    set.iter()
        .map(|(field, e)| Ok((field.clone(), eval(e, &Scope::Row(row))?)))
        .collect()
}

/// Column labels, rows and the number of rows before `LIMIT`.
pub type SelectOutput = (Vec<String>, Vec<Vec<Value>>, u64);

/// Runs a `SELECT` over a collection's records. `fields` lists the columns
/// `SELECT *` shows. Returns the column labels, at most `MAX_SQL_ROWS` rows
/// and the number of rows before `LIMIT`.
pub fn run_select(
    select: &Select,
    fields: &[String],
    records: &[SqlRow],
) -> Result<SelectOutput, String> {
    let mut matching = Vec::new();
    for row in records {
        if sql_matches(select.filter.as_ref(), row)? {
            matching.push(row);
        }
    }
    let grouped =
        !select.group_by.is_empty() || select.columns.iter().any(|c| has_aggregate(&c.expr));
    if select.columns.is_empty() && grouped {
        return Err("SELECT * cannot be grouped; name the columns".into());
    }
    if select.filter.as_ref().is_some_and(has_aggregate) {
        return Err("WHERE cannot use COUNT, SUM, AVG, MIN or MAX".into());
    }
    let columns: Vec<SqlColumn> = if select.columns.is_empty() {
        fields
            .iter()
            .map(|f| SqlColumn {
                expr: SqlExpr::Field(f.clone()),
                label: f.clone(),
                alias: false,
            })
            .collect()
    } else {
        select.columns.clone()
    };
    // ORDER BY may name a column alias or position.
    let order: Vec<(SqlExpr, bool)> = select
        .order_by
        .iter()
        .map(|(e, desc)| {
            let resolved = match e {
                SqlExpr::Literal(Value::Number(n)) => n
                    .as_u64()
                    .and_then(|i| columns.get((i as usize).checked_sub(1)?))
                    .map(|c| c.expr.clone())
                    .ok_or_else(|| format!("ORDER BY {n}: there is no column {n}"))?,
                SqlExpr::Field(name) => columns
                    .iter()
                    .find(|c| c.alias && c.label == *name)
                    .map(|c| c.expr.clone())
                    .unwrap_or_else(|| e.clone()),
                _ => e.clone(),
            };
            Ok((resolved, *desc))
        })
        .collect::<Result<_, String>>()?;
    let labels: Vec<String> = columns.iter().map(|c| c.label.clone()).collect();
    let sort = |keys: &mut Vec<(Vec<Value>, usize)>| {
        keys.sort_by(|(a, i), (b, j)| {
            for ((x, y), (_, desc)) in a.iter().zip(b).zip(&order) {
                // Empty values last, either direction.
                let o = match (x.is_null(), y.is_null()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => return Ordering::Greater,
                    (false, true) => return Ordering::Less,
                    _ => compare(x, y).unwrap_or(Ordering::Equal),
                };
                let o = if *desc { o.reverse() } else { o };
                if o != Ordering::Equal {
                    return o;
                }
            }
            i.cmp(j)
        });
    };
    let limit = select.limit.unwrap_or(usize::MAX).min(MAX_SQL_ROWS);
    if !grouped {
        let mut keys = Vec::with_capacity(matching.len());
        for (i, row) in matching.iter().enumerate() {
            let scope = Scope::Row(row);
            keys.push((
                order
                    .iter()
                    .map(|(e, _)| eval(e, &scope))
                    .collect::<Result<_, _>>()?,
                i,
            ));
        }
        if !order.is_empty() {
            sort(&mut keys);
        }
        let total = matching.len() as u64;
        let mut rows = Vec::new();
        for (_, i) in keys.into_iter().skip(select.offset).take(limit) {
            let scope = Scope::Row(matching[i]);
            rows.push(output(&columns, &scope)?);
        }
        return Ok((labels, rows, total));
    }
    let mut groups: Vec<Vec<&SqlRow>> = Vec::new();
    let mut index = HashMap::<String, usize>::new();
    for row in &matching {
        let scope = Scope::Row(row);
        let key = select
            .group_by
            .iter()
            .map(|e| eval(e, &scope))
            .collect::<Result<Vec<_>, _>>()?;
        let key = serde_json::to_string(&key).unwrap_or_default();
        let slot = *index.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[slot].push(row);
    }
    // Without GROUP BY, aggregates cover all records, even none.
    if select.group_by.is_empty() && groups.is_empty() {
        groups.push(Vec::new());
    }
    let mut keys = Vec::with_capacity(groups.len());
    for (i, group) in groups.iter().enumerate() {
        let scope = Scope::Group(group);
        keys.push((
            order
                .iter()
                .map(|(e, _)| eval(e, &scope))
                .collect::<Result<_, _>>()?,
            i,
        ));
    }
    if !order.is_empty() {
        sort(&mut keys);
    }
    let total = groups.len() as u64;
    let mut rows = Vec::new();
    for (_, i) in keys.into_iter().skip(select.offset).take(limit) {
        let scope = Scope::Group(&groups[i]);
        rows.push(output(&columns, &scope)?);
    }
    Ok((labels, rows, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records() -> Vec<SqlRow> {
        [
            json!({"id":"1","aar":2019,"navn":"Solbakken","kommune":"Oslo","fte":3.5}),
            json!({"id":"2","aar":2023,"navn":"Fjellstua","kommune":"Bergen","fte":10}),
            json!({"id":"3","aar":"2024","navn":"Måken","kommune":"Oslo","fte":7}),
            json!({"id":"4","aar":2024,"navn":"Tunet","kommune":null}),
        ]
        .into_iter()
        .map(|v| v.as_object().unwrap().clone())
        .collect()
    }
    fn select(sql: &str) -> (Vec<String>, Vec<Vec<Value>>, u64) {
        let Statement::Select(s) = parse_sql(sql).unwrap() else {
            panic!()
        };
        let fields = vec!["id".into(), "aar".into(), "navn".into()];
        run_select(&s, &fields, &records()).unwrap()
    }

    #[test]
    fn selects_filter_sort_and_limit() {
        let (cols, rows, total) = select(
            "SELECT navn, fte * 2 AS double FROM barnehage WHERE aar >= 2023 ORDER BY double DESC LIMIT 2",
        );
        assert_eq!(cols, ["navn", "double"]);
        assert_eq!(total, 3);
        assert_eq!(
            rows,
            vec![
                vec![json!("Fjellstua"), json!(20)],
                vec![json!("Måken"), json!(14)]
            ]
        );
        let (cols, rows, _) =
            select("select * from barnehage where navn like 'm%' or kommune is null order by 1");
        assert_eq!(cols, ["id", "aar", "navn"]);
        assert_eq!(
            rows,
            vec![
                vec![json!("3"), json!("2024"), json!("Måken")],
                vec![json!("4"), json!(2024), json!("Tunet")]
            ]
        );
        let (_, rows, _) = select(
            "SELECT navn FROM b WHERE kommune IN ('Oslo', 'Bergen') AND NOT aar BETWEEN 2020 AND 2023 ORDER BY navn",
        );
        assert_eq!(rows, vec![vec![json!("Måken")], vec![json!("Solbakken")]]);
        // Unknown (empty) values never match, not even with <>.
        let (_, rows, _) = select("SELECT navn FROM b WHERE kommune <> 'Oslo'");
        assert_eq!(rows, vec![vec![json!("Fjellstua")]]);
    }

    #[test]
    fn groups_and_aggregates() {
        let (cols, rows, total) = select(
            "SELECT kommune, COUNT(*) AS antall, SUM(fte), AVG(fte), MAX(aar) FROM b GROUP BY kommune ORDER BY antall DESC, kommune",
        );
        assert_eq!(
            cols,
            ["kommune", "antall", "SUM(fte)", "AVG(fte)", "MAX(aar)"]
        );
        assert_eq!(total, 3);
        assert_eq!(
            rows[0],
            vec![
                json!("Oslo"),
                json!(2),
                json!(10.5),
                json!(5.25),
                json!("2024")
            ]
        );
        // Empty groups go last.
        assert_eq!(rows[2][0], Value::Null);
        let (_, rows, _) = select("SELECT COUNT(*), SUM(fte) FROM b WHERE aar > 3000");
        assert_eq!(rows, vec![vec![json!(0), json!(0)]]);
        let (_, rows, _) =
            select("SELECT ROUND(AVG(fte), 1), UPPER('x') || LENGTH(navn) FROM b WHERE id = '2'");
        assert_eq!(rows, vec![vec![json!(10), json!("X9")]]);
    }

    #[test]
    fn updates_and_deletes_parse_with_conditions() {
        let Statement::Update { table, set, filter } = parse_sql(
            "UPDATE pensjon-2024 SET fte = fte + 1, navn = UPPER(navn) WHERE aar < 2020;",
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(table, "pensjon-2024");
        let row = &records()[0];
        assert!(sql_matches(filter.as_ref(), row).unwrap());
        assert_eq!(
            sql_assignments(&set, row).unwrap(),
            json!({"fte":4.5,"navn":"SOLBAKKEN"})
                .as_object()
                .unwrap()
                .clone()
        );
        let statement = parse_sql("DELETE FROM \"order\" WHERE \"select\" = 1").unwrap();
        assert_eq!(statement.table(), "order");
        assert_eq!(statement.fields(), ["select"]);
    }

    #[test]
    fn errors_say_what_and_where() {
        let cases = [
            (
                "DROP TABLE x",
                "start with SELECT, UPDATE or DELETE",
                Some(0),
            ),
            ("SELECT navn FROM", "expected a collection name", Some(16)),
            (
                "SELECT navn FROM b WHERE aar >",
                "the statement ends too early",
                Some(30),
            ),
            (
                "DELETE FROM b WHERE navn = 'x",
                "this quote is never closed",
                Some(27),
            ),
            ("UPDATE b SET order = 1", "ORDER is a keyword", Some(13)),
            ("SELECT FOO(navn) FROM b", "unknown function FOO", Some(7)),
            (
                "SELECT navn FROM b; DELETE FROM b",
                "unexpected text after the statement",
                Some(20),
            ),
        ];
        for (sql, message, at) in cases {
            let e = parse_sql(sql).unwrap_err();
            assert!(e.message.contains(message), "{sql}: {e}");
            assert_eq!(e.at, at, "{sql}");
        }
        let Statement::Delete { filter, .. } =
            parse_sql("DELETE FROM b WHERE COUNT(*) > 1").unwrap()
        else {
            panic!()
        };
        assert!(
            sql_matches(filter.as_ref(), &records()[0])
                .unwrap_err()
                .contains("use it in SELECT")
        );
        let Statement::Delete { filter, .. } = parse_sql("DELETE FROM b WHERE navn").unwrap()
        else {
            panic!()
        };
        assert!(
            sql_matches(filter.as_ref(), &records()[0])
                .unwrap_err()
                .contains("not a condition")
        );
    }
}
