//! Value conversions, comparisons and criteria shared by the evaluator and functions.
use chrono::{Duration, NaiveDate};
use serde_json::{Value, json};

/// Values that compare equal in criteria share a key: numbers and numeric
/// text by value, other text case-insensitively.
pub(super) fn match_key(v: &Value) -> String {
    match as_number(v) {
        // -0 and 0 are the same value.
        Some(n) => if n == 0.0 { 0.0 } else { n }.to_string(),
        None => text_value(v).to_lowercase(),
    }
}
pub(super) fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}
pub(super) fn number(n: f64) -> Result<Value, String> {
    serde_json::Number::from_f64(n)
        .map(Value::Number)
        .ok_or("#NUM!".into())
}
pub(super) fn num(v: &Value) -> Result<f64, String> {
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
pub(super) fn truth(v: &Value) -> Result<bool, String> {
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
pub(super) fn flat(v: &Value, out: &mut Vec<Value>) {
    if let Value::Array(a) = v {
        for v in a {
            flat(v, out);
        }
    } else {
        out.push(v.clone());
    }
}
pub(super) fn compare(a: &Value, b: &Value, op: &str) -> Result<bool, String> {
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
pub(super) fn index(v: &Value, min: usize) -> Result<usize, String> {
    let n = num(v)?;
    if n < min as f64 || n > 1_000_000.0 {
        return Err("#VALUE! index out of range".into());
    }
    Ok(n.trunc() as usize)
}
pub(super) fn date(v: &Value) -> Result<NaiveDate, String> {
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
pub(super) fn criterion(v: &Value, c: &Value) -> Result<bool, String> {
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
// Excel wildcards: * = any sequence, ? = one character, ~ = escape.
pub(super) fn wildcard(text: &str, pattern: &str, prefix: bool) -> Result<bool, String> {
    wildcard_chars(
        &text.chars().collect::<Vec<_>>(),
        pattern,
        prefix,
        &mut 1_000_000,
    )
}
pub(super) fn wildcard_chars(
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
