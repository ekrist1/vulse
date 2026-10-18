//! Parses formula text into a bounded expression tree.
use super::FUNCTIONS;
use super::value::number;
use serde_json::{Value, json};
use std::collections::HashSet;

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
pub(super) fn parse(formula: &str) -> Result<Expr, String> {
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

pub(super) fn arity(name: &str, n: usize) -> Result<(), String> {
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
pub(super) fn refs(e: &Expr, out: &mut HashSet<String>) {
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
