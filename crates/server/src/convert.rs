//! Converting stored values when a field changes type, such as text that
//! holds numbers becoming a number field.

use serde_json::Value;
use vulse_model::{FieldDef, FieldType};

/// Converts one stored value from `from`'s type to `to`'s. `Ok(None)` means
/// the value becomes empty (blank text, for example). Errors describe why a
/// value cannot be converted.
pub fn convert_value(
    value: &Value,
    from: &FieldDef,
    to: &FieldDef,
) -> Result<Option<Value>, String> {
    if value.is_null() {
        return Ok(None);
    }
    if from.kind == to.kind && from.items == to.items {
        return Ok(Some(value.clone()));
    }
    let text = || match value {
        Value::String(text) => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    };
    match to.kind {
        FieldType::String => match (value, from.kind) {
            (Value::String(_) | Value::Number(_) | Value::Bool(_), _) => {
                Ok(text().map(Value::String))
            }
            // Rich text keeps its words.
            (_, FieldType::PortableText) => {
                Ok(Some(Value::String(vulse_model::display_value(value))))
            }
            _ => Err("only single values can become text".into()),
        },
        FieldType::Number => match value {
            Value::Bool(flag) => Ok(Some(Value::from(u8::from(*flag)))),
            _ => parse(to.kind, text()),
        },
        FieldType::Boolean | FieldType::Date => parse(to.kind, text()),
        // Free-form JSON accepts any value as it is.
        FieldType::Json => Ok(Some(value.clone())),
        // IDs stay as text; the usual checks confirm the records or files exist.
        FieldType::Reference | FieldType::Asset if value.is_string() => {
            Ok(text().filter(|t| !t.is_empty()).map(Value::String))
        }
        _ => Err(format!(
            "cannot be converted automatically to {}",
            kind_name(to.kind)
        )),
    }
}

fn parse(kind: FieldType, text: Option<String>) -> Result<Option<Value>, String> {
    match text {
        Some(text) if text.is_empty() => Ok(None),
        Some(text) => crate::import::convert(kind, &text).map(Some),
        None => Err(format!(
            "is not a single value that can become {}",
            kind_name(kind)
        )),
    }
}

pub fn kind_name(kind: FieldType) -> &'static str {
    match kind {
        FieldType::String => "text",
        FieldType::Number => "a number",
        FieldType::Boolean => "true/false",
        FieldType::Date => "a date",
        FieldType::Json => "JSON",
        FieldType::PortableText => "rich text",
        FieldType::Asset => "a file",
        FieldType::Reference => "a reference",
        FieldType::Array => "a list",
        FieldType::Calculated => "a calculated value",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(kind: &str) -> FieldDef {
        serde_json::from_value(json!({"name":"f","type":kind})).unwrap()
    }

    #[test]
    fn values_convert_between_common_types() {
        let (text, number, boolean, date) = (
            field("string"),
            field("number"),
            field("boolean"),
            field("date"),
        );
        let ok = |v: Value, from: &FieldDef, to: &FieldDef| convert_value(&v, from, to).unwrap();
        assert_eq!(ok(json!("12"), &text, &number), Some(json!(12)));
        assert_eq!(ok(json!(" 3.5 "), &text, &number), Some(json!(3.5)));
        assert_eq!(ok(json!("  "), &text, &number), None);
        assert_eq!(ok(json!(12), &number, &text), Some(json!("12")));
        assert_eq!(ok(json!(true), &boolean, &number), Some(json!(1)));
        assert_eq!(ok(json!("yes"), &text, &boolean), Some(json!(true)));
        assert_eq!(ok(json!("0"), &text, &boolean), Some(json!(false)));
        assert_eq!(
            ok(json!("2026-01-05"), &text, &date),
            Some(json!("2026-01-05"))
        );
        assert_eq!(
            ok(json!("2026-01-05"), &date, &text),
            Some(json!("2026-01-05"))
        );
        let blocks = json!([{"_type":"block","_key":"a","style":"normal","markDefs":[],"children":[{"_type":"span","_key":"s","text":"Hello","marks":[]}]}]);
        assert_eq!(
            ok(blocks, &field("portable_text"), &text),
            Some(json!("Hello"))
        );
        for (value, to) in [
            (json!("12a"), &number),
            (json!("1,5"), &number),
            (json!("05.01.2026"), &date),
            (json!("maybe"), &boolean),
        ] {
            assert!(convert_value(&value, &text, to).is_err(), "{value}");
        }
        assert!(convert_value(&json!(["a"]), &field("json"), &text).is_err());
        assert!(convert_value(&json!("a"), &text, &field("portable_text")).is_err());
    }
}
