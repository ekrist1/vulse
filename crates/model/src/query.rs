use crate::{Blueprint, DocumentView, FieldType};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordFilter {
    pub field: String,
    pub op: String,
    #[serde(default)]
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecordQuery {
    pub search: String,
    pub filters: Vec<RecordFilter>,
    pub sort_field: String,
    pub descending: bool,
    pub group_field: String,
    pub limit: u32,
    pub offset: u32,
}
impl Default for RecordQuery {
    fn default() -> Self {
        Self {
            search: String::new(),
            filters: vec![],
            sort_field: "created_at".into(),
            descending: true,
            group_field: String::new(),
            limit: 25,
            offset: 0,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedViewWrite {
    pub name: String,
    pub query: RecordQuery,
    pub columns: Vec<String>,
    /// Grouped totals shown in the view's Summary mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<crate::SummarySpec>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedView {
    pub id: String,
    #[serde(flatten)]
    pub config: SavedViewWrite,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordPage {
    pub items: Vec<DocumentView>,
    pub total: u64,
    pub limit: u32,
    pub offset: u32,
}

pub fn validate_query(blueprint: &Blueprint, query: &RecordQuery) -> Result<(), String> {
    if !(1..=100).contains(&query.limit)
        || query.offset > 1_000_000
        || query.search.chars().count() > 200
        || query.filters.len() > 20
    {
        return Err("query exceeds search, filter or pagination limits".into());
    }
    if !["created_at", "updated_at", "id"].contains(&query.sort_field.as_str())
        && !blueprint
            .fields
            .iter()
            .any(|field| field.name == query.sort_field)
    {
        return Err("unknown sort field".into());
    }
    if !query.group_field.is_empty()
        && !blueprint
            .fields
            .iter()
            .any(|field| field.name == query.group_field)
    {
        return Err("unknown grouping field".into());
    }
    for filter in &query.filters {
        let field = blueprint
            .fields
            .iter()
            .find(|field| field.name == filter.field)
            .ok_or("unknown filter field")?;
        if ![
            "contains",
            "eq",
            "neq",
            "gt",
            "gte",
            "lt",
            "lte",
            "empty",
            "not_empty",
        ]
        .contains(&filter.op.as_str())
        {
            return Err("unknown filter operator".into());
        }
        if ["empty", "not_empty"].contains(&filter.op.as_str()) {
            continue;
        }
        let is_date_value = filter.value.as_str().is_some_and(crate::is_date);
        if ["gt", "gte", "lt", "lte"].contains(&filter.op.as_str()) {
            let comparable = match field.kind {
                FieldType::Number | FieldType::Calculated => filter.value.is_number(),
                FieldType::Date => is_date_value,
                _ => false,
            };
            if !comparable {
                return Err(
                    "comparisons require a number or date field and a matching value".into(),
                );
            }
        }
        if matches!(filter.op.as_str(), "eq" | "neq")
            && ((field.kind == FieldType::Number && !filter.value.is_number())
                || (field.kind == FieldType::Boolean && !filter.value.is_boolean())
                || (field.kind == FieldType::Date && !is_date_value))
        {
            return Err("filter value does not match field type".into());
        }
        if filter.value.to_string().len() > 2000 {
            return Err("filter value is too long".into());
        }
    }
    Ok(())
}
pub fn validate_view(blueprint: &Blueprint, view: &SavedViewWrite) -> Result<(), String> {
    validate_query(blueprint, &view.query)?;
    if let Some(summary) = &view.summary {
        crate::validate_summary(blueprint, summary)?;
    }
    if view.name.trim().is_empty() || view.name.chars().count() > 100 {
        return Err("view name must contain 1–100 characters".into());
    }
    let mut columns = std::collections::HashSet::new();
    for column in &view.columns {
        if !columns.insert(column) || !blueprint.fields.iter().any(|field| field.name == *column) {
            return Err("view columns must be unique existing fields".into());
        }
    }
    Ok(())
}

pub fn display_value(value: &Value) -> String {
    if let Some(blocks) = value
        .as_array()
        .filter(|items| items.iter().any(|item| item.get("_type").is_some()))
    {
        fn text(block: &Value) -> String {
            if block["_type"] == "block" {
                return block["children"]
                    .as_array()
                    .map(|spans| {
                        spans
                            .iter()
                            .filter_map(|span| span["text"].as_str())
                            .collect::<String>()
                    })
                    .unwrap_or_default();
            }
            let mut pieces = vec![];
            if let Some(title) = block["title"].as_str() {
                pieces.push(title.to_string());
            }
            if let Some(content) = block["content"].as_array() {
                pieces.extend(content.iter().map(text));
            }
            if let Some(items) = block["items"].as_array() {
                for item in items {
                    if let Some(label) = item["label"].as_str() {
                        pieces.push(label.into());
                    }
                    if let Some(content) = item["content"].as_array() {
                        pieces.extend(content.iter().map(text));
                    }
                }
            }
            pieces.join(" ")
        }
        return blocks.iter().map(text).collect::<Vec<_>>().join(" ");
    }
    crate::text_value(value)
}
