use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::identity::{check_collection_name, is_date, is_field_name, is_uuid};
use crate::portable_text::validate_portable_text;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blueprint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_field: Option<String>,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    pub name: String,
    #[serde(default)]
    pub required: bool,
    #[serde(rename = "type")]
    pub kind: FieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<ArrayItems>,
    /// Only in schema updates: the saved field this one renames. Its values,
    /// formula references, title setting and saved views move to `name`.
    /// Never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Calculated,
    String,
    Number,
    Boolean,
    /// A calendar date stored as `YYYY-MM-DD`, without time or time zone.
    Date,
    Json,
    PortableText,
    Asset,
    Reference,
    Array,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrayItems {
    String,
    Number,
    Asset,
    Reference,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocumentRef {
    pub collection: String,
    pub document_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckedDocument {
    pub references: Vec<DocumentRef>,
    pub asset_ids: Vec<String>,
}

pub fn validate_blueprint(blueprint: &Blueprint) -> Result<(), String> {
    let mut names = HashSet::new();
    for field in &blueprint.fields {
        for (key, value, limit) in [
            ("label", &field.label, 200),
            ("description", &field.description, 1000),
            ("example", &field.example, 500),
        ] {
            if value.as_ref().is_some_and(|value| {
                value.chars().count() > limit || (key == "label" && value.trim().is_empty())
            }) {
                return Err(format!(
                    "{}: {key} must {}contain at most {limit} characters",
                    field.name,
                    if key == "label" {
                        "be nonempty and "
                    } else {
                        ""
                    }
                ));
            }
        }
        if !is_field_name(&field.name) {
            return Err(format!("invalid field name '{}'", field.name));
        }
        if !names.insert(field.name.as_str()) {
            return Err(format!("duplicate field '{}'", field.name));
        }
        if field.kind != FieldType::Calculated && field.formula.is_some() {
            return Err(format!(
                "{}: only calculated fields can set formula",
                field.name
            ));
        }
        if field.kind == FieldType::Calculated && field.required {
            return Err(format!(
                "{}: calculated fields cannot be required",
                field.name
            ));
        }
        match field.kind {
            FieldType::Reference => {
                require_collection(field)?;
                if field.items.is_some() {
                    return Err(format!("field '{}' cannot set items", field.name));
                }
            }
            FieldType::Array => {
                let Some(items) = field.items else {
                    return Err(format!("field '{}' requires items", field.name));
                };
                if items == ArrayItems::Reference {
                    require_collection(field)?;
                } else if field.collection.is_some() {
                    return Err(format!("field '{}' cannot set collection", field.name));
                }
            }
            _ => {
                if field.collection.is_some() || field.items.is_some() {
                    return Err(format!(
                        "field '{}' cannot set collection or items",
                        field.name
                    ));
                }
            }
        }
    }
    if let Some(title) = &blueprint.title_field
        && !blueprint.fields.iter().any(|field| {
            field.name == *title
                && matches!(
                    field.kind,
                    FieldType::String
                        | FieldType::Number
                        | FieldType::Boolean
                        | FieldType::Date
                        | FieldType::Calculated
                )
        }) {
            return Err(
                "record title must select a text, number, boolean, date or calculated field".into(),
            );
        }
    crate::validate_formulas(blueprint)
}

/// Takes the `renamed_from` markers out of an updated blueprint and returns
/// the renames as old name → new name. Formula references and the title field
/// in `blueprint` follow the renames.
///
/// A rename must come from a field in `saved` whose name the updated blueprint
/// no longer uses, and each saved field can be renamed only once.
pub fn take_field_renames(
    blueprint: &mut Blueprint,
    saved: &Blueprint,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut renames = std::collections::BTreeMap::new();
    let names: HashSet<String> = blueprint.fields.iter().map(|f| f.name.clone()).collect();
    for field in &mut blueprint.fields {
        let Some(old) = field.renamed_from.take() else {
            continue;
        };
        if old == field.name {
            continue;
        }
        if !saved.fields.iter().any(|f| f.name == old) {
            return Err(format!(
                "field '{}' is renamed from '{old}', which is not a saved field",
                field.name
            ));
        }
        if names.contains(&old) {
            return Err(format!(
                "field '{old}' cannot be renamed to '{}' while a field named '{old}' remains",
                field.name
            ));
        }
        if renames.insert(old.clone(), field.name.clone()).is_some() {
            return Err(format!("field '{old}' is renamed more than once"));
        }
    }
    if renames.is_empty() {
        return Ok(renames);
    }
    for field in &mut blueprint.fields {
        if let Some(formula) = &field.formula {
            field.formula = Some(crate::rename_formula_fields(formula, &renames));
        }
    }
    if let Some(title) = &blueprint.title_field
        && let Some(next) = renames.get(title) {
            blueprint.title_field = Some(next.clone());
        }
    Ok(renames)
}

impl FieldDef {
    pub fn display_label(&self) -> &str {
        self.label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
            .unwrap_or(&self.name)
    }
}

pub fn record_title(blueprint: &Blueprint, body: &Value, id: &str) -> String {
    let field = blueprint.title_field.as_deref().or_else(|| {
        blueprint
            .fields
            .iter()
            .find(|field| field.kind == FieldType::String)
            .map(|field| field.name.as_str())
    });
    let title = field
        .and_then(|field| body.get(field))
        .filter(|value| !value.is_null())
        .map(crate::text_value)
        .unwrap_or_default();
    if title.trim().is_empty() {
        format!(
            "Untitled record · {}",
            id.chars().take(8).collect::<String>()
        )
    } else {
        title.chars().take(120).collect()
    }
}

pub fn reference_collections(blueprint: &Blueprint) -> Vec<&str> {
    blueprint
        .fields
        .iter()
        .filter(|field| {
            field.kind == FieldType::Reference
                || (field.kind == FieldType::Array && field.items == Some(ArrayItems::Reference))
        })
        .filter_map(|field| field.collection.as_deref())
        .collect()
}

pub fn validate_document(blueprint: &Blueprint, body: &Value) -> Result<CheckedDocument, String> {
    validate_blueprint(blueprint)?;
    let obj = body.as_object().ok_or("document must be a JSON object")?;
    let known: HashSet<&str> = blueprint
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    for key in obj.keys() {
        if !known.contains(key.as_str()) {
            return Err(format!("unknown field '{key}'"));
        }
    }
    let mut checked = CheckedDocument {
        references: Vec::new(),
        asset_ids: Vec::new(),
    };
    for field in &blueprint.fields {
        if field.kind == FieldType::Calculated {
            continue;
        }
        match obj.get(&field.name) {
            None | Some(Value::Null) => {
                if field.required {
                    return Err(format!("{} is required", field.name));
                }
            }
            Some(value) => validate_field(field, value, &mut checked)?,
        }
    }
    Ok(checked)
}

fn require_collection(field: &FieldDef) -> Result<(), String> {
    let Some(collection) = field.collection.as_deref() else {
        return Err(format!("field '{}' requires a collection", field.name));
    };
    check_collection_name(collection)
        .map_err(|_| format!("field '{}' has an invalid collection name", field.name))
}

fn validate_field(
    field: &FieldDef,
    value: &Value,
    checked: &mut CheckedDocument,
) -> Result<(), String> {
    match field.kind {
        FieldType::Calculated => Ok(()),
        FieldType::String => expect_string(field, value).map(|_| ()),
        FieldType::Number => {
            if value.is_number() {
                Ok(())
            } else {
                Err(format!("{} must be a number", field.name))
            }
        }
        FieldType::Boolean => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(format!("{} must be a boolean", field.name))
            }
        }
        FieldType::Date => {
            if value.as_str().is_some_and(is_date) {
                Ok(())
            } else {
                Err(format!("{} must be a date as YYYY-MM-DD", field.name))
            }
        }
        FieldType::Json => Ok(()),
        FieldType::PortableText => {
            let assets =
                validate_portable_text(value).map_err(|err| format!("{}: {err}", field.name))?;
            checked.asset_ids.extend(assets);
            Ok(())
        }
        FieldType::Asset => {
            checked.asset_ids.push(expect_id(field, value)?);
            Ok(())
        }
        FieldType::Reference => {
            let collection = field
                .collection
                .clone()
                .ok_or_else(|| format!("field '{}' requires a collection", field.name))?;
            checked.references.push(DocumentRef {
                collection,
                document_id: expect_id(field, value)?,
            });
            Ok(())
        }
        FieldType::Array => {
            let items = value
                .as_array()
                .ok_or_else(|| format!("{} must be an array", field.name))?;
            let item_kind = field
                .items
                .ok_or_else(|| format!("field '{}' requires items", field.name))?;
            for (index, item) in items.iter().enumerate() {
                match item_kind {
                    ArrayItems::String => {
                        if !item.is_string() {
                            return Err(format!("{}[{index}] must be a string", field.name));
                        }
                    }
                    ArrayItems::Number => {
                        if !item.is_number() {
                            return Err(format!("{}[{index}] must be a number", field.name));
                        }
                    }
                    ArrayItems::Asset => {
                        checked
                            .asset_ids
                            .push(expect_id_at(&field.name, index, item)?);
                    }
                    ArrayItems::Reference => {
                        let collection = field.collection.clone().ok_or_else(|| {
                            format!("field '{}' requires a collection", field.name)
                        })?;
                        checked.references.push(DocumentRef {
                            collection,
                            document_id: expect_id_at(&field.name, index, item)?,
                        });
                    }
                }
            }
            Ok(())
        }
    }
}

fn expect_string<'a>(field: &FieldDef, value: &'a Value) -> Result<&'a str, String> {
    value
        .as_str()
        .ok_or_else(|| format!("{} must be a string", field.name))
}

fn expect_id(field: &FieldDef, value: &Value) -> Result<String, String> {
    let id = expect_string(field, value)?;
    if is_uuid(id) {
        Ok(id.to_string())
    } else {
        Err(format!("{} must be an id", field.name))
    }
}

fn expect_id_at(name: &str, index: usize, value: &Value) -> Result<String, String> {
    let Some(id) = value.as_str() else {
        return Err(format!("{name}[{index}] must be a string"));
    };
    if is_uuid(id) {
        Ok(id.to_string())
    } else {
        Err(format!("{name}[{index}] must be an id"))
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn renames_move_formula_references_and_titles() {
        let renames = std::collections::BTreeMap::from([
            ("price".to_string(), "unit_price".to_string()),
            ("qty".to_string(), "price".to_string()),
        ]);
        assert_eq!(
            crate::rename_formula_fields(r#"=[price]*[qty]&"[price]""ok"&[other]"#, &renames),
            r#"=[unit_price]*[price]&"[price]""ok"&[other]"#
        );
        assert_eq!(
            crate::rename_formula_fields("=[price", &renames),
            "=[unit_price"
        );

        let saved: Blueprint = serde_json::from_value(serde_json::json!({
            "title_field": "price",
            "fields": [
                {"name": "price", "type": "number"},
                {"name": "name", "type": "string"},
                {"name": "double", "type": "calculated", "formula": "=[price]*2"}
            ]
        }))
        .unwrap();
        let mut next = saved.clone();
        next.fields[0].name = "unit_price".into();
        next.fields[0].renamed_from = Some("price".into());
        let renames = take_field_renames(&mut next, &saved).unwrap();
        assert_eq!(renames.get("price").map(String::as_str), Some("unit_price"));
        assert!(next.fields.iter().all(|f| f.renamed_from.is_none()));
        assert_eq!(next.fields[2].formula.as_deref(), Some("=[unit_price]*2"));
        assert_eq!(next.title_field.as_deref(), Some("unit_price"));
        assert!(validate_blueprint(&next).is_ok());

        // Renaming back to the saved name is not a rename.
        let mut same = saved.clone();
        same.fields[0].renamed_from = Some("price".into());
        assert!(take_field_renames(&mut same, &saved).unwrap().is_empty());

        for (from, keep_old) in [("missing", false), ("price", true)] {
            let mut bad = saved.clone();
            bad.fields[1].name = "label".into();
            bad.fields[1].renamed_from = Some(from.into());
            if !keep_old {
                bad.fields[0].name = "price".into();
            }
            assert!(take_field_renames(&mut bad, &saved).is_err(), "{from}");
        }
        let mut twice = saved.clone();
        twice.fields[0].name = "a".into();
        twice.fields[0].renamed_from = Some("price".into());
        twice.fields[1].name = "b".into();
        twice.fields[1].renamed_from = Some("price".into());
        assert!(take_field_renames(&mut twice, &saved).is_err());
    }
    use super::*;
    use serde_json::json;

    #[test]
    fn labels_are_metadata_and_titles_use_the_selected_field() {
        let blueprint: Blueprint = serde_json::from_value(json!({"title_field":"price", "fields":[{"name":"name","type":"string","label":"Product name","description":"Customer-facing name","example":"Notebook"},{"name":"price","type":"number","label":"Unit price"}]})).unwrap();
        validate_blueprint(&blueprint).unwrap();
        assert_eq!(blueprint.fields[0].display_label(), "Product name");
        validate_document(&blueprint, &json!({"name":"Notebook","price":12})).unwrap();
        assert_eq!(
            record_title(
                &blueprint,
                &json!({"name":"Notebook","price":12}),
                "abcdef123"
            ),
            "12"
        );
        assert_eq!(
            record_title(&blueprint, &json!({}), "abcdef123"),
            "Untitled record · abcdef12"
        );
        let mut invalid = blueprint.clone();
        invalid.title_field = Some("missing".into());
        assert!(validate_blueprint(&invalid).is_err());
        invalid = blueprint.clone();
        invalid.fields[0].label = Some(" ".into());
        assert!(validate_blueprint(&invalid).is_err());
        let legacy: Blueprint =
            serde_json::from_value(json!({"fields":[{"name":"name","type":"string"}]})).unwrap();
        assert_eq!(
            record_title(&legacy, &json!({"name":"Notebook"}), "id"),
            "Notebook"
        );
    }

    fn string_field(name: &str, required: bool) -> FieldDef {
        FieldDef {
            label: None,
            description: None,
            example: None,
            formula: None,
            name: name.into(),
            required,
            kind: FieldType::String,
            collection: None,
            items: None,
            renamed_from: None,
        }
    }

    #[test]
    fn parses_a_blueprint_and_rejects_a_breaking_change() {
        let blueprint: Blueprint = serde_json::from_value(json!({
            "fields": [
                {"name": "title", "type": "string", "required": true},
                {"name": "body", "type": "portable_text"},
                {"name": "author", "type": "reference", "collection": "authors"},
                {"name": "tags", "type": "array", "items": "string"}
            ]
        }))
        .unwrap();
        validate_blueprint(&blueprint).unwrap();
        assert!(reference_collections(&blueprint).contains(&"authors"));

        let body = json!({"title": "Hello"});
        validate_document(&blueprint, &body).unwrap();

        let renamed = Blueprint {
            title_field: None,
            fields: vec![string_field("name", true)],
        };
        let err = validate_document(&renamed, &body).unwrap_err();
        assert!(err.contains("unknown field"));

        let extended = Blueprint {
            title_field: None,
            fields: vec![string_field("title", true), string_field("subtitle", false)],
        };
        validate_document(&extended, &body).unwrap();
    }

    #[test]
    fn collects_references_and_asset_ids() {
        let blueprint = Blueprint {
            title_field: None,
            fields: vec![
                FieldDef {
                    label: None,
                    description: None,
                    example: None,
                    formula: None,
                    name: "cover".into(),
                    required: true,
                    kind: FieldType::Asset,
                    collection: None,
                    items: None,
                    renamed_from: None,
                },
                FieldDef {
                    label: None,
                    description: None,
                    example: None,
                    formula: None,
                    name: "author".into(),
                    required: true,
                    kind: FieldType::Reference,
                    collection: Some("authors".into()),
                    items: None,
                    renamed_from: None,
                },
            ],
        };
        let asset = "550e8400-e29b-41d4-a716-446655440000";
        let author = "550e8400-e29b-41d4-a716-446655440001";
        let checked =
            validate_document(&blueprint, &json!({"cover": asset, "author": author})).unwrap();
        assert_eq!(checked.asset_ids, vec![asset]);
        assert_eq!(checked.references[0].collection, "authors");
        assert_eq!(checked.references[0].document_id, author);
        assert!(validate_document(&blueprint, &json!({"cover": asset})).is_err());
    }
}
