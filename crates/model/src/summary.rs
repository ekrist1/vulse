//! Grouped summaries of a collection: group records by fields, aggregate
//! values per group, and derive more values with formulas over those results.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::identity::is_field_name;
use crate::{Blueprint, FieldDef, FieldType, RecordQuery};

pub const MAX_GROUP_FIELDS: usize = 3;
pub const MAX_MEASURES: usize = 20;
pub const MAX_CHART_SERIES: usize = 4;
/// Groups returned per summary. More groups are counted but not listed.
pub const MAX_SUMMARY_GROUPS: usize = 1000;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummarySpec {
    /// Fields that form a group. Empty means one group for all records.
    #[serde(default)]
    pub group_by: Vec<SummaryGroup>,
    pub measures: Vec<SummaryMeasure>,
    /// Group or measure name to order groups by. Defaults to the groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(default)]
    pub descending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart: Option<SummaryChart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryGroup {
    pub field: String,
    /// Date fields only: group by day, month or year.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket: Option<DateBucket>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateBucket {
    Day,
    Month,
    Year,
}

impl DateBucket {
    /// Characters of a `YYYY-MM-DD` date that form the bucket key.
    pub fn width(self) -> usize {
        match self {
            DateBucket::Day => 10,
            DateBucket::Month => 7,
            DateBucket::Year => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryOp {
    Sum,
    Average,
    Min,
    Max,
    /// Records in the group, or records with a value when `field` is set.
    Count,
    CountUnique,
    /// A formula over the other measures, such as `=[revenue]/[staff]`.
    Formula,
}

impl SummaryOp {
    pub const ALL: [SummaryOp; 7] = [
        SummaryOp::Sum,
        SummaryOp::Average,
        SummaryOp::Min,
        SummaryOp::Max,
        SummaryOp::Count,
        SummaryOp::CountUnique,
        SummaryOp::Formula,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SummaryOp::Sum => "sum",
            SummaryOp::Average => "average",
            SummaryOp::Min => "min",
            SummaryOp::Max => "max",
            SummaryOp::Count => "count",
            SummaryOp::CountUnique => "count_unique",
            SummaryOp::Formula => "formula",
        }
    }

    pub fn parse(value: &str) -> Option<SummaryOp> {
        SummaryOp::ALL.into_iter().find(|op| op.as_str() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            SummaryOp::Sum => "Sum",
            SummaryOp::Average => "Average",
            SummaryOp::Min => "Minimum",
            SummaryOp::Max => "Maximum",
            SummaryOp::Count => "Count",
            SummaryOp::CountUnique => "Count unique",
            SummaryOp::Formula => "Formula",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryMeasure {
    /// Result column name, also how formulas refer to it: `[name]`.
    pub name: String,
    pub op: SummaryOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartKind {
    Bar,
    Line,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryChart {
    pub kind: ChartKind,
    /// Measure names to plot, one series each.
    pub measures: Vec<String>,
}

/// Body of `POST /collections/{id}/summary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryRequest {
    /// Search and filters applied before grouping. Sorting and paging are ignored.
    #[serde(default)]
    pub query: RecordQuery,
    pub summary: SummarySpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryRow {
    /// One value per group field; `null` for records without a value.
    pub group: Vec<Value>,
    /// Readable group values, such as a referenced record's title.
    pub labels: Vec<String>,
    /// Records in the group.
    pub count: u64,
    pub values: Map<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryResult {
    pub rows: Vec<SummaryRow>,
    /// The same measures over all matching records.
    pub totals: SummaryRow,
    pub group_count: u64,
    /// True when more than `MAX_SUMMARY_GROUPS` groups exist.
    pub truncated: bool,
}

/// Expand direct collection aggregates into private SQL measures. Repeated aggregates
/// share a measure; the browser never needs the underlying records.
/// Existing summary names take precedence, preserving scalar formula semantics.
pub fn compile_summary(blueprint: &Blueprint, spec: &SummarySpec) -> Result<SummarySpec, String> {
    use crate::FormulaExpression as E;
    fn rewrite(
        e: E,
        bp: &Blueprint,
        original: &SummarySpec,
        out: &mut SummarySpec,
    ) -> Result<E, String> {
        if let E::Call(function, args) = &e {
            let op = match function.as_str() {
                "SUM" => Some(SummaryOp::Sum),
                "AVERAGE" => Some(SummaryOp::Average),
                "MIN" => Some(SummaryOp::Min),
                "MAX" => Some(SummaryOp::Max),
                "COUNT" => Some(SummaryOp::Count),
                _ => None,
            };
            if let (Some(op), [E::Field(field)]) = (op, args.as_slice()) {
                if !original.measures.iter().any(|m| m.name == *field) {
                    if !bp.fields.iter().any(|f| f.name == *field) {
                        return Err(format!("unknown collection field [{field}]"));
                    }
                    if let Some(existing) = out
                        .measures
                        .iter()
                        .find(|m| m.op == op && m.field.as_ref() == Some(field))
                    {
                        return Ok(E::Field(existing.name.clone()));
                    }
                    if out.measures.len() - original.measures.len() >= 100 {
                        return Err("use at most 100 distinct collection aggregates".into());
                    }
                    let mut index = out.measures.len();
                    let name = loop {
                        let name = format!("summary_internal_{index}");
                        if !out.measures.iter().any(|m| m.name == name) {
                            break name;
                        }
                        index += 1;
                    };
                    out.measures.push(SummaryMeasure {
                        name: name.clone(),
                        op,
                        field: Some(field.clone()),
                        formula: None,
                    });
                    return Ok(E::Field(name));
                }
            }
        }
        Ok(match e {
            E::Field(ref name)
                if !name.contains('.') && !original.measures.iter().any(|m| m.name == *name) =>
            {
                return Err(if bp.fields.iter().any(|f| f.name == *name) {
                    format!(
                        "collection field [{name}] needs an aggregate, for example SUM([{name}])"
                    )
                } else {
                    format!("unknown summary value [{name}]")
                });
            }
            E::Unary(op, a) => E::Unary(op, Box::new(rewrite(*a, bp, original, out)?)),
            E::Binary(op, a, b) => E::Binary(
                op,
                Box::new(rewrite(*a, bp, original, out)?),
                Box::new(rewrite(*b, bp, original, out)?),
            ),
            E::Call(name, args) => E::Call(
                name,
                args.into_iter()
                    .map(|a| rewrite(a, bp, original, out))
                    .collect::<Result<_, _>>()?,
            ),
            other => other,
        })
    }
    let mut out = spec.clone();
    for (index, measure) in spec.measures.iter().enumerate() {
        if measure.op == SummaryOp::Formula
            && let Some(formula) = &measure.formula
        {
            let original =
                crate::inspect_formula(formula).map_err(|e| format!("{}: {e}", measure.name))?;
            let expr = rewrite(original.clone(), blueprint, spec, &mut out)
                .map_err(|e| format!("{}: {e}", measure.name))?;
            if expr != original {
                out.measures[index].formula =
                    Some(format!("={}", render_summary_expression(&expr)));
            }
        }
    }
    Ok(out)
}

fn render_summary_expression(e: &crate::FormulaExpression) -> String {
    use crate::FormulaExpression as E;
    match e {
        E::Literal(Value::String(s)) => format!("\"{}\"", s.replace('"', "\"\"")),
        E::Literal(v) => v.to_string(),
        E::Field(n) => format!("[{n}]"),
        E::Unary(op, a) => format!("({op}{})", render_summary_expression(a)),
        E::Binary(op, a, b) => format!(
            "({}{op}{})",
            render_summary_expression(a),
            render_summary_expression(b)
        ),
        E::Call(n, args) => format!(
            "{n}({})",
            args.iter()
                .map(render_summary_expression)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// Checks a summary against the collection's fields.
pub fn validate_summary(blueprint: &Blueprint, spec: &SummarySpec) -> Result<(), String> {
    if spec.measures.is_empty() || spec.measures.len() > MAX_MEASURES {
        return Err("add 1 to 20 summary values".into());
    }
    if let Some(sort) = &spec.sort
        && !spec.measures.iter().any(|m| m.name == *sort)
        && !spec.group_by.iter().any(|g| g.field == *sort)
    {
        return Err(format!("cannot sort by unknown summary column '{sort}'"));
    }
    if let Some(chart) = &spec.chart
        && let Some(missing) = chart
            .measures
            .iter()
            .find(|name| !spec.measures.iter().any(|m| m.name == **name))
    {
        return Err(format!("the chart uses unknown summary value '{missing}'"));
    }
    let expanded = compile_summary(blueprint, spec)?;
    validate_expanded_summary(blueprint, &expanded)
}

fn validate_expanded_summary(blueprint: &Blueprint, spec: &SummarySpec) -> Result<(), String> {
    let field = |name: &str| blueprint.fields.iter().find(|f| f.name == name);
    if spec.group_by.len() > MAX_GROUP_FIELDS {
        return Err("group by at most 3 fields".into());
    }
    let mut groups = HashSet::new();
    for group in &spec.group_by {
        let def =
            field(&group.field).ok_or_else(|| format!("unknown group field '{}'", group.field))?;
        if !groups.insert(&group.field) {
            return Err(format!("'{}' is grouped twice", group.field));
        }
        if !matches!(
            def.kind,
            FieldType::String
                | FieldType::Number
                | FieldType::Boolean
                | FieldType::Date
                | FieldType::Reference
                | FieldType::Calculated
        ) {
            return Err(format!(
                "'{}' cannot be grouped; choose a text, number, true/false, date, reference or calculated field",
                group.field
            ));
        }
        if group.bucket.is_some() && def.kind != FieldType::Date {
            return Err(format!(
                "'{}' is not a date field, so it cannot group by day, month or year",
                group.field
            ));
        }
    }
    let mut names = HashSet::new();
    for measure in &spec.measures {
        if !is_field_name(&measure.name) {
            return Err(format!(
                "summary value name '{}' must start with a letter and use letters, numbers or underscores",
                measure.name
            ));
        }
        if !names.insert(&measure.name) {
            return Err(format!("summary value '{}' is defined twice", measure.name));
        }
        match measure.op {
            SummaryOp::Formula => {
                if measure.field.is_some() {
                    return Err(format!(
                        "'{}' is a formula and takes no field",
                        measure.name
                    ));
                }
                if measure
                    .formula
                    .as_deref()
                    .is_none_or(|f| f.trim().is_empty())
                {
                    return Err(format!("'{}' needs a formula", measure.name));
                }
            }
            SummaryOp::Count if measure.field.is_none() => {}
            op => {
                let name = measure
                    .field
                    .as_deref()
                    .ok_or_else(|| format!("'{}' needs a field", measure.name))?;
                let def = field(name).ok_or_else(|| format!("unknown field '{name}'"))?;
                let numeric = matches!(def.kind, FieldType::Number | FieldType::Calculated);
                let allowed = match op {
                    SummaryOp::Sum | SummaryOp::Average => numeric,
                    SummaryOp::Min | SummaryOp::Max => numeric || def.kind == FieldType::Date,
                    _ => !matches!(def.kind, FieldType::PortableText | FieldType::Json),
                };
                if !allowed {
                    return Err(format!(
                        "{} cannot be used on '{name}'",
                        op.label().to_lowercase()
                    ));
                }
                if measure.formula.is_some() {
                    return Err(format!("'{}' is not a formula", measure.name));
                }
            }
        }
    }
    crate::validate_formulas(&formula_blueprint(spec))?;
    for measure in &spec.measures {
        if let Some((collection, field)) = measure
            .formula
            .as_deref()
            .and_then(|f| crate::formula_lookups(f).into_iter().next())
        {
            return Err(format!(
                "'{}': {}",
                measure.name,
                lookup_message(&collection, &field)
            ));
        }
    }
    if let Some(sort) = &spec.sort
        && !names.contains(sort)
        && !groups.contains(sort)
    {
        return Err(format!("cannot sort by unknown summary column '{sort}'"));
    }
    if let Some(chart) = &spec.chart {
        if chart.measures.is_empty() || chart.measures.len() > MAX_CHART_SERIES {
            return Err("a chart shows 1 to 4 summary values".into());
        }
        if let Some(missing) = chart.measures.iter().find(|m| !names.contains(m)) {
            return Err(format!("the chart uses unknown summary value '{missing}'"));
        }
    }
    Ok(())
}

/// Aggregates as plain number fields and formula measures as calculated
/// fields, so the formula engine can evaluate them in dependency order.
fn formula_blueprint(spec: &SummarySpec) -> Blueprint {
    Blueprint {
        title_field: None,
        fields: spec
            .measures
            .iter()
            .map(|m| FieldDef {
                label: None,
                description: None,
                example: None,
                formula: m.formula.clone(),
                name: m.name.clone(),
                required: false,
                kind: if m.op == SummaryOp::Formula {
                    FieldType::Calculated
                } else {
                    FieldType::Json
                },
                collection: None,
                items: None,
                renamed_from: None,
                number_format: None,
                options: None,
                renamed_options: None,
            })
            .collect(),
    }
}

/// Compile once per summary request, then reuse for every group and totals.
pub fn summary_formulas(spec: &SummarySpec) -> crate::CompiledFormulas {
    crate::CompiledFormulas::new(&formula_blueprint(spec))
}

/// Fills in formula measures from the aggregate values of one group.
pub fn apply_summary_formulas(spec: &SummarySpec, row: &mut SummaryRow) {
    if !spec.measures.iter().any(|m| m.op == SummaryOp::Formula) {
        return;
    }
    let (body, errors) =
        crate::calculate_document(&formula_blueprint(spec), &Value::Object(row.values.clone()));
    if let Value::Object(values) = body {
        row.values = values;
    }
    row.errors.extend(errors);
}

/// Renames field references in a summary after fields are renamed.
pub fn rename_summary_fields(spec: &mut SummarySpec, renames: &BTreeMap<String, String>) {
    for group in &mut spec.group_by {
        if let Some(next) = renames.get(&group.field) {
            group.field = next.clone();
        }
    }
    let summary_names: HashSet<_> = spec.measures.iter().map(|m| m.name.clone()).collect();
    let collection_renames = renames
        .iter()
        .filter(|(name, _)| !summary_names.contains(*name))
        .map(|(a, b)| (a.clone(), b.clone()))
        .collect();
    for measure in &mut spec.measures {
        if let Some(formula) = &mut measure.formula {
            *formula = crate::rename_formula_fields(formula, &collection_renames);
        }
        if let Some(next) = measure.field.as_ref().and_then(|f| renames.get(f)) {
            measure.field = Some(next.clone());
        }
    }
    let sorting_measure = spec
        .sort
        .as_ref()
        .is_some_and(|s| spec.measures.iter().any(|m| m.name == *s));
    if !sorting_measure
        && let Some(next) = spec.sort.as_ref().and_then(|s| renames.get(s))
        && spec.group_by.iter().any(|g| g.field == *next)
    {
        spec.sort = Some(next.clone());
    }
}

/// Why a summary formula cannot use `[collection.field]`, and what to do.
pub fn lookup_message(collection: &str, field: &str) -> String {
    format!(
        "summary formulas can only use this summary's values, not [{collection}.{field}]. Add a calculated field that reads {collection} (for example with SUMIFS) to the records, then summarize that field."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn blueprint() -> Blueprint {
        serde_json::from_value(json!({"fields":[
            {"name":"name","type":"string"},
            {"name":"revenue","type":"number"},
            {"name":"employees","type":"number"},
            {"name":"opened","type":"date"},
            {"name":"notes","type":"portable_text"}
        ]}))
        .unwrap()
    }

    fn spec(value: Value) -> SummarySpec {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn formulas_combine_aggregates_in_dependency_order() {
        let spec = spec(json!({
            "group_by":[{"field":"name"}],
            "measures":[
                {"name":"per_pi","op":"formula","formula":"=[total_revenue]/3.14"},
                {"name":"total_revenue","op":"sum","field":"revenue"},
                {"name":"average_employees","op":"average","field":"employees"},
                {"name":"product","op":"formula","formula":"=[total_revenue]*[average_employees]"},
                {"name":"broken","op":"formula","formula":"=[total_revenue]/0"}
            ],
            "sort":"total_revenue","descending":true,
            "chart":{"kind":"line","measures":["total_revenue","product"]}
        }));
        validate_summary(&blueprint(), &spec).unwrap();
        let mut row = SummaryRow {
            group: vec![json!("Barnehage1")],
            labels: vec!["Barnehage1".into()],
            count: 2,
            values: json!({"total_revenue":3000,"average_employees":4.5})
                .as_object()
                .unwrap()
                .clone(),
            errors: BTreeMap::new(),
        };
        apply_summary_formulas(&spec, &mut row);
        assert_eq!(row.values["product"].as_f64(), Some(13500.0));
        assert!((row.values["per_pi"].as_f64().unwrap() - 955.414).abs() < 0.001);
        assert!(row.errors.contains_key("broken"), "{:?}", row.errors);
    }

    #[test]
    fn collection_aggregates_compile_validate_and_rename() {
        let bp = blueprint();
        let mut spec = spec(json!({"measures":[
            {"name":"double","op":"formula","formula":"=SUM([revenue])*2"},
            {"name":"ratio","op":"formula","formula":"=ROUND(SUM([revenue])/AVERAGE([employees]),2)"},
            {"name":"label","op":"formula","formula":"=IF([double]>0,\"SUM([revenue])\",\"empty\")"}
        ]}));
        validate_summary(&bp, &spec).unwrap();
        let compiled = compile_summary(&bp, &spec).unwrap();
        assert_eq!(
            compiled.measures.len(),
            5,
            "repeated SUM should share one SQL aggregate"
        );
        assert_eq!(compiled.measures[3].field.as_deref(), Some("revenue"));
        let mut row = SummaryRow {
            group: vec![],
            labels: vec![],
            count: 2,
            values: Map::from_iter([
                (compiled.measures[3].name.clone(), json!(300)),
                (compiled.measures[4].name.clone(), json!(3)),
            ]),
            errors: Default::default(),
        };
        apply_summary_formulas(&compiled, &mut row);
        assert_eq!(row.values["double"], json!(600.0));
        assert_eq!(row.values["ratio"], json!(100.0));
        assert_eq!(row.values["label"], "SUM([revenue])");
        rename_summary_fields(
            &mut spec,
            &BTreeMap::from([("revenue".into(), "income".into())]),
        );
        assert_eq!(
            spec.measures[0].formula.as_deref(),
            Some("=SUM([income])*2")
        );
        assert!(
            spec.measures[2]
                .formula
                .as_ref()
                .unwrap()
                .contains("SUM([revenue])")
        );
    }

    #[test]
    fn collection_aggregates_reject_ambiguous_or_invalid_inputs() {
        for (formula, message) in [
            ("=[revenue]*2", "needs an aggregate"),
            ("=SUM([missing])", "unknown collection field"),
            ("=SUM([name])", "sum cannot be used"),
            ("=SUM([revenue]*2)", "needs an aggregate"),
            ("=MEDIAN([revenue])", "needs an aggregate"),
            ("=SUM([revenue],[employees])", "needs an aggregate"),
        ] {
            let s = spec(json!({"measures":[{"name":"result","op":"formula","formula":formula}]}));
            assert!(
                validate_summary(&blueprint(), &s)
                    .unwrap_err()
                    .contains(message),
                "{formula}"
            );
        }
        let s = spec(json!({"measures":[
            {"name":"revenue","op":"count"},
            {"name":"result","op":"formula","formula":"=SUM([revenue])*2"}
        ]}));
        assert_eq!(compile_summary(&blueprint(), &s).unwrap().measures.len(), 2);
    }

    #[test]
    fn invalid_summaries_explain_the_problem() {
        let bp = blueprint();
        for (value, message) in [
            (json!({"measures":[]}), "1 to 20"),
            (
                json!({"measures":[{"name":"t","op":"sum","field":"name"}]}),
                "sum cannot be used",
            ),
            (
                json!({"measures":[{"name":"t","op":"sum"}]}),
                "needs a field",
            ),
            (
                json!({"measures":[{"name":"t","op":"count","field":"notes"}]}),
                "count cannot be used",
            ),
            (
                json!({"measures":[{"name":"1t","op":"count"}]}),
                "must start with a letter",
            ),
            (
                json!({"measures":[{"name":"t","op":"count"},{"name":"t","op":"count"}]}),
                "twice",
            ),
            (
                json!({"measures":[{"name":"t","op":"formula","formula":"=[missing]*2"}]}),
                "missing",
            ),
            (
                json!({"measures":[{"name":"a","op":"formula","formula":"=[b]"},{"name":"b","op":"formula","formula":"=[a]"}]}),
                "ircular",
            ),
            (
                json!({"group_by":[{"field":"name","bucket":"month"}],"measures":[{"name":"t","op":"count"}]}),
                "not a date field",
            ),
            (
                json!({"group_by":[{"field":"notes"}],"measures":[{"name":"t","op":"count"}]}),
                "cannot be grouped",
            ),
            (
                json!({"measures":[{"name":"t","op":"count"}],"sort":"x"}),
                "unknown summary column",
            ),
            (
                json!({"measures":[{"name":"t","op":"count"}],"chart":{"kind":"bar","measures":["x"]}}),
                "unknown summary value",
            ),
        ] {
            let error = validate_summary(&bp, &spec(value.clone())).unwrap_err();
            assert!(error.contains(message), "{value}: {error}");
        }
        let dated = spec(json!({"group_by":[{"field":"opened","bucket":"year"}],
            "measures":[{"name":"first","op":"min","field":"opened"},{"name":"n","op":"count_unique","field":"name"}]}));
        validate_summary(&bp, &dated).unwrap();
    }
}
