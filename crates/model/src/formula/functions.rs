//! The spreadsheet function library, applied to evaluated arguments.
use super::FUNCTIONS;
use super::parser::arity;
use super::value::{
    compare, criterion, date, flat, index, num, number, text_value, truth, wildcard, wildcard_chars,
};
use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{Value, json};

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
        "SUMIFS" | "COUNTIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS" | "XLOOKUP" => Err(format!(
            "{name} is not available in SQL; use WHERE and GROUP BY"
        )),
        _ => function(&name, args),
    }
}
pub(super) fn function(n: &str, a: &[Value]) -> Result<Value, String> {
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
