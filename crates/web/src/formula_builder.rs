use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use crate::formula_input::{FormulaInput, Suggestion};
use crate::lookup_builder::{LookupBuilder, LookupDraft};
use vulse_model::{
    ArrayItems, Blueprint, CollectionView, DocumentView, FieldDef, FieldType,
    FormulaExpression as Expr,
};
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Number,
    Text,
    Boolean,
    Values,
    Array,
    Any,
    Condition,
    Date,
}
#[derive(Clone)]
struct Argument {
    label: &'static str,
    kind: Kind,
    default: Option<&'static str>,
    optional: bool,
}
fn arg(label: &'static str, kind: Kind) -> Argument {
    Argument {
        label,
        kind,
        default: None,
        optional: false,
    }
}
fn fixed(label: &'static str, kind: Kind, value: &'static str) -> Argument {
    Argument {
        label,
        kind,
        default: Some(value),
        optional: false,
    }
}
fn optional(label: &'static str, kind: Kind, value: &'static str) -> Argument {
    Argument {
        label,
        kind,
        default: Some(value),
        optional: true,
    }
}
#[derive(Clone)]
struct FunctionHelp {
    description: &'static str,
    category: &'static str,
    example: &'static str,
    args: Vec<Argument>,
    variadic: bool,
}
fn help(name: &str) -> FunctionHelp {
    use Kind::*;
    let (description, category, example, args) = match name {
        "SUM" => (
            "Add numbers or the numbers in a list.",
            "Numbers",
            "SUM([price], [shipping])",
            vec![arg("Value to add", Values)],
        ),
        "AVERAGE" => (
            "Find the average of numbers or a numeric list.",
            "Numbers",
            "AVERAGE([scores])",
            vec![arg("Values to average", Values)],
        ),
        "MIN" => (
            "Find the smallest number.",
            "Numbers",
            "MIN([prices])",
            vec![arg("Values to compare", Values)],
        ),
        "MAX" => (
            "Find the largest number.",
            "Numbers",
            "MAX([prices])",
            vec![arg("Values to compare", Values)],
        ),
        "COUNT" => (
            "Count numeric values.",
            "Numbers",
            "COUNT([scores])",
            vec![arg("Values to count", Any)],
        ),
        "COUNTA" => (
            "Count values that are not empty.",
            "Numbers",
            "COUNTA([tags])",
            vec![arg("Values to count", Any)],
        ),
        "COUNTBLANK" => (
            "Count empty values.",
            "Numbers",
            "COUNTBLANK([answers])",
            vec![arg("Values to check", Any)],
        ),
        "PRODUCT" => (
            "Multiply numbers together.",
            "Numbers",
            "PRODUCT([price], [quantity])",
            vec![arg("Value to multiply", Values)],
        ),
        "MEDIAN" => (
            "Find the middle number in a set of values.",
            "Numbers",
            "MEDIAN([scores])",
            vec![arg("Values to compare", Values)],
        ),
        "ABS" => (
            "Remove the sign from a number (its absolute value).",
            "Numbers",
            "ABS([balance])",
            vec![arg("Number", Number)],
        ),
        "ROUND" => (
            "Round a number to a chosen number of decimal places.",
            "Numbers",
            "ROUND([price], 2)",
            vec![
                arg("Value to round", Number),
                fixed("Decimal places", Number, "2"),
            ],
        ),
        "ROUNDUP" => (
            "Round away from zero.",
            "Numbers",
            "ROUNDUP([price], 2)",
            vec![
                arg("Value to round", Number),
                fixed("Decimal places", Number, "2"),
            ],
        ),
        "ROUNDDOWN" => (
            "Round towards zero.",
            "Numbers",
            "ROUNDDOWN([price], 2)",
            vec![
                arg("Value to round", Number),
                fixed("Decimal places", Number, "2"),
            ],
        ),
        "INT" => (
            "Round down to the nearest whole number.",
            "Numbers",
            "INT([price])",
            vec![arg("Number", Number)],
        ),
        "MOD" => (
            "Return the remainder after division.",
            "Numbers",
            "MOD([quantity], 3)",
            vec![arg("Number", Number), fixed("Divide by", Number, "3")],
        ),
        "POWER" => (
            "Raise a number to a power.",
            "Numbers",
            "POWER([width], 2)",
            vec![arg("Number", Number), fixed("Power", Number, "2")],
        ),
        "SQRT" => (
            "Find the square root of a non-negative number.",
            "Numbers",
            "SQRT([area])",
            vec![arg("Number", Number)],
        ),
        "CEILING" => (
            "Round up to a multiple.",
            "Numbers",
            "CEILING([price], 5)",
            vec![arg("Number", Number), fixed("Multiple", Number, "1")],
        ),
        "FLOOR" => (
            "Round down to a multiple.",
            "Numbers",
            "FLOOR([price], 5)",
            vec![arg("Number", Number), fixed("Multiple", Number, "1")],
        ),
        "IF" => (
            "Return one value when a condition is true, another when it is false.",
            "Logic",
            "IF([price] > 100, \"Premium\", \"Standard\")",
            vec![
                arg("Condition", Condition),
                fixed("When true", Any, "Yes"),
                fixed("Otherwise", Any, "No"),
            ],
        ),
        "IFERROR" => (
            "Use a fallback when a value fails to calculate.",
            "Logic",
            "IFERROR([ratio], 0)",
            vec![arg("Value to check", Any), fixed("Fallback", Any, "0")],
        ),
        "AND" => (
            "Check whether all conditions are true.",
            "Logic",
            "AND([approved], [paid])",
            vec![arg("Condition", Condition)],
        ),
        "OR" => (
            "Check whether any condition is true.",
            "Logic",
            "OR([approved], [paid])",
            vec![arg("Condition", Condition)],
        ),
        "NOT" => (
            "Reverse a true or false value.",
            "Logic",
            "NOT([approved])",
            vec![arg("Condition", Condition)],
        ),
        "CONCAT" | "CONCATENATE" => (
            "Join values into one text value.",
            "Text",
            "CONCAT([first_name], \" \", [last_name])",
            vec![arg("Text to combine", Any)],
        ),
        "TEXTJOIN" => (
            "Join values with a separator between them.",
            "Text",
            "TEXTJOIN(\" \", TRUE, [first_name], [last_name])",
            vec![
                fixed("Separator", Text, " "),
                fixed("Skip empty values", Boolean, "true"),
                arg("Text to combine", Any),
            ],
        ),
        "LEFT" => (
            "Take characters from the beginning of text.",
            "Text",
            "LEFT([name], 3)",
            vec![arg("Text", Text), optional("Characters", Number, "1")],
        ),
        "RIGHT" => (
            "Take characters from the end of text.",
            "Text",
            "RIGHT([name], 3)",
            vec![arg("Text", Text), optional("Characters", Number, "1")],
        ),
        "MID" => (
            "Take characters from a position in text. Positions start at 1.",
            "Text",
            "MID([name], 2, 3)",
            vec![
                arg("Text", Text),
                fixed("Start position", Number, "1"),
                fixed("Characters", Number, "3"),
            ],
        ),
        "LEN" => (
            "Count the characters in text.",
            "Text",
            "LEN([name])",
            vec![arg("Text", Text)],
        ),
        "LOWER" => (
            "Convert text to lowercase.",
            "Text",
            "LOWER([name])",
            vec![arg("Text", Text)],
        ),
        "UPPER" => (
            "Convert text to uppercase.",
            "Text",
            "UPPER([name])",
            vec![arg("Text", Text)],
        ),
        "PROPER" => (
            "Capitalize the first letter of each word.",
            "Text",
            "PROPER([name])",
            vec![arg("Text", Text)],
        ),
        "TRIM" => (
            "Remove surrounding spaces and repeated spaces.",
            "Text",
            "TRIM([name])",
            vec![arg("Text", Text)],
        ),
        "SUBSTITUTE" => (
            "Replace matching text with new text.",
            "Text",
            "SUBSTITUTE([name], \"old\", \"new\")",
            vec![
                arg("Text", Text),
                fixed("Text to find", Text, "old"),
                fixed("Replacement", Text, "new"),
                optional("Occurrence (omit for all)", Number, "1"),
            ],
        ),
        "REPLACE" => (
            "Replace characters at a position in text.",
            "Text",
            "REPLACE([code], 1, 2, \"AB\")",
            vec![
                arg("Text", Text),
                fixed("Start position", Number, "1"),
                fixed("Characters to replace", Number, "1"),
                fixed("Replacement", Text, "new"),
            ],
        ),
        "FIND" => (
            "Find text, matching letter case. Positions start at 1.",
            "Text",
            "FIND(\"@\", [email])",
            vec![
                fixed("Text to find", Text, "@"),
                arg("Search in", Text),
                optional("Start position", Number, "1"),
            ],
        ),
        "SEARCH" => (
            "Find text without matching letter case.",
            "Text",
            "SEARCH(\"hello\", [message])",
            vec![
                fixed("Text to find", Text, "hello"),
                arg("Search in", Text),
                optional("Start position", Number, "1"),
            ],
        ),
        "EXACT" => (
            "Check whether two text values match exactly, including case.",
            "Text",
            "EXACT([name], \"Ada\")",
            vec![arg("First text", Text), fixed("Second text", Text, "Ada")],
        ),
        "VALUE" => (
            "Convert numeric text to a number.",
            "Numbers",
            "VALUE([price_text])",
            vec![arg("Numeric text", Text)],
        ),
        "DATE" => (
            "Build a date from year, month, and day; returns an Excel serial number.",
            "Dates",
            "DATE(2026, 10, 11)",
            vec![
                fixed("Year", Number, "2026"),
                fixed("Month", Number, "1"),
                fixed("Day", Number, "1"),
            ],
        ),
        "YEAR" => (
            "Get the year from an ISO date or Excel serial number.",
            "Dates",
            "YEAR([date])",
            vec![arg("Date (YYYY-MM-DD)", Date)],
        ),
        "MONTH" => (
            "Get the month from an ISO date or Excel serial number.",
            "Dates",
            "MONTH([date])",
            vec![arg("Date (YYYY-MM-DD)", Date)],
        ),
        "DAY" => (
            "Get the day from an ISO date or Excel serial number.",
            "Dates",
            "DAY([date])",
            vec![arg("Date (YYYY-MM-DD)", Date)],
        ),
        "SUMIF" => (
            "Add list values that match a condition.",
            "Lists",
            "SUMIF([scores], \">10\")",
            vec![
                arg("List to check", Array),
                fixed("Match condition", Text, ">0"),
                Argument {
                    label: "List to sum",
                    kind: Array,
                    default: None,
                    optional: true,
                },
            ],
        ),
        "COUNTIF" => (
            "Count list values that match a condition.",
            "Lists",
            "COUNTIF([tags], \"done\")",
            vec![
                arg("List to check", Array),
                fixed("Match condition", Text, "done"),
            ],
        ),
        "AVERAGEIF" => (
            "Average list values that match a condition.",
            "Lists",
            "AVERAGEIF([scores], \">10\")",
            vec![
                arg("List to check", Array),
                fixed("Match condition", Text, ">0"),
                Argument {
                    label: "List to average",
                    kind: Array,
                    default: None,
                    optional: true,
                },
            ],
        ),
        "INDEX" => (
            "Get a value from a list. Positions start at 1.",
            "Lists",
            "INDEX([tags], 1)",
            vec![arg("List", Array), fixed("Position", Number, "1")],
        ),
        "MATCH" => (
            "Find a value's position in a list. Use mode 0 for an exact match.",
            "Lists",
            "MATCH(\"done\", [tags], 0)",
            vec![
                fixed("Value to find", Any, "done"),
                arg("Search list", Array),
                optional("Match mode (0 = exact)", Number, "0"),
            ],
        ),
        "SUMIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS" => (
            match name {
                "SUMIFS" => {
                    "Add the values of rows that match every condition. Lists can be another collection's field, like [orders.amount]."
                }
                "AVERAGEIFS" => {
                    "Average the values of rows that match every condition. Lists can be another collection's field, like [orders.amount]."
                }
                "MINIFS" => {
                    "Find the smallest value among rows that match every condition. Lists can be another collection's field."
                }
                _ => {
                    "Find the largest value among rows that match every condition. Lists can be another collection's field."
                }
            },
            "Lists",
            match name {
                "SUMIFS" => "SUMIFS([orders.amount], [orders.customer], [id], [orders.year], 2025)",
                "AVERAGEIFS" => "AVERAGEIFS([orders.amount], [orders.customer], [id])",
                "MINIFS" => "MINIFS([orders.amount], [orders.customer], [id])",
                _ => "MAXIFS([orders.year], [orders.customer], [id])",
            },
            vec![
                arg("Values", Array),
                arg("List to check", Array),
                fixed("Condition", Text, ">0"),
                Argument {
                    label: "Second list to check",
                    kind: Array,
                    default: None,
                    optional: true,
                },
                optional("Second condition", Text, ">0"),
            ],
        ),
        "COUNTIFS" => (
            "Count rows that match every condition. Lists can be another collection's field, like [orders.customer].",
            "Lists",
            "COUNTIFS([orders.customer], [id])",
            vec![
                arg("List to check", Array),
                fixed("Condition", Text, ">0"),
                Argument {
                    label: "Second list to check",
                    kind: Array,
                    default: None,
                    optional: true,
                },
                optional("Second condition", Text, ">0"),
            ],
        ),
        "XLOOKUP" => (
            "Find a value in a list and return the value in the same row of another list. Lists can be another collection's field.",
            "Lists",
            "XLOOKUP([org_nr], [companies.org_nr], [companies.name], \"Not found\")",
            vec![
                fixed("Value to find", Any, "done"),
                arg("List to search", Array),
                arg("List to return from", Array),
                optional("If not found", Any, "Not found"),
            ],
        ),
        _ => unreachable!("all functions have help"),
    };
    FunctionHelp {
        description,
        category,
        example,
        args,
        variadic: matches!(
            name,
            "SUM"
                | "AVERAGE"
                | "MIN"
                | "MAX"
                | "COUNT"
                | "COUNTA"
                | "COUNTBLANK"
                | "PRODUCT"
                | "MEDIAN"
                | "AND"
                | "OR"
                | "CONCAT"
                | "CONCATENATE"
                | "TEXTJOIN"
        ),
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Binding {
    source: String,
    value: String,
    comparison: String,
    compare_value: String,
}
impl Binding {
    fn empty() -> Self {
        Self {
            source: String::new(),
            value: String::new(),
            comparison: ">".into(),
            compare_value: "0".into(),
        }
    }
    fn fixed(kind: Kind, value: &str) -> Self {
        let source = match kind {
            Kind::Number | Kind::Values => "number",
            Kind::Boolean | Kind::Condition => "boolean",
            Kind::Array => "json",
            Kind::Any if value == "0" => "number",
            _ => "text",
        };
        Self {
            source: source.into(),
            value: value.into(),
            ..Self::empty()
        }
    }
    fn from_expr(e: &Expr) -> Option<Self> {
        Some(match e {
            Expr::Field(name) => Self {
                source: format!("field:{name}"),
                comparison: "truthy".into(),
                ..Self::empty()
            },
            Expr::Literal(v) => Self {
                source: if v.is_number() {
                    "number"
                } else if v.is_boolean() {
                    "boolean"
                } else if v.is_string() {
                    "text"
                } else {
                    "json"
                }
                .into(),
                value: if let Some(s) = v.as_str() {
                    s.into()
                } else {
                    v.as_f64()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| v.to_string())
                },
                ..Self::empty()
            },
            Expr::Unary('-', value) if matches!(value.as_ref(),Expr::Literal(v) if v.is_number()) =>
            {
                let mut b = Self::from_expr(value)?;
                b.value = format!("-{}", b.value);
                b
            }
            Expr::Binary(op, left, right)
                if ["=", "<>", "!=", ">", "<", ">=", "<="].contains(&op.as_str()) =>
            {
                let Expr::Field(name) = left.as_ref() else {
                    return None;
                };
                let Expr::Literal(v) = right.as_ref() else {
                    return None;
                };
                Self {
                    source: format!("field:{name}"),
                    comparison: op.clone(),
                    compare_value: v.to_string(),
                    ..Self::empty()
                }
            }
            _ => return None,
        })
    }
    fn expression(&self, argument: &Argument, fields: &[FieldDef]) -> Result<String, String> {
        if let Some(name) = self.source.strip_prefix("field:") {
            let field = fields
                .iter()
                .find(|f| f.name == name)
                .ok_or_else(|| format!("{}: select a field that still exists.", argument.label))?;
            if !compatible(argument.kind, field) {
                return Err(format!(
                    "{}: choose {}.",
                    argument.label,
                    kind_hint(argument.kind)
                ));
            }
            let field_expr = format!("[{name}]");
            if argument.kind == Kind::Condition && self.comparison != "truthy" {
                let value: Value = serde_json::from_str(&self.compare_value)
                    .map_err(|_| format!("{}: enter a comparison value.", argument.label))?;
                return Ok(format!(
                    "{field_expr} {} {}",
                    self.comparison,
                    formula_literal(&value)
                ));
            }
            return Ok(field_expr);
        }
        if self.source.is_empty() {
            return Err(format!(
                "{}: choose a field or a fixed value.",
                argument.label
            ));
        }
        let value = match self.source.as_str() {
            "text" => json!(self.value),
            "number" => serde_json::from_str::<Value>(&self.value)
                .ok()
                .filter(Value::is_number)
                .ok_or_else(|| format!("{}: enter a number.", argument.label))?,
            "boolean" => json!(self.value == "true"),
            "json" => serde_json::from_str::<Value>(&self.value)
                .map_err(|_| format!("{}: enter valid JSON, such as [1, 2, 3].", argument.label))?,
            _ => return Err("Choose an input source.".into()),
        };
        if argument.kind == Kind::Array && !value.is_array() {
            return Err(format!(
                "{}: enter a list, such as [1, 2, 3].",
                argument.label
            ));
        }
        let valid = match argument.kind {
            Kind::Number | Kind::Values => value.is_number(),
            Kind::Boolean | Kind::Condition => value.is_boolean(),
            Kind::Text => value.is_string(),
            Kind::Date => value.is_string() || value.is_number(),
            Kind::Array => false,
            Kind::Any => value.is_string() || value.is_number() || value.is_boolean(),
        };
        if !valid {
            return Err(format!(
                "{}: choose {}.",
                argument.label,
                kind_hint(argument.kind)
            ));
        }
        Ok(formula_literal(&value))
    }
}
fn formula_literal(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else if let Some(value) = value.as_bool() {
        if value { "TRUE".into() } else { "FALSE".into() }
    } else {
        value
            .as_f64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| value.to_string())
    }
}
fn preview_text(value: &Value) -> String {
    if value.is_null() {
        "Empty".into()
    } else if value.as_str() == Some("") {
        "Empty text".into()
    } else if let Some(number) = value.as_f64() {
        number.to_string()
    } else if let Some(boolean) = value.as_bool() {
        if boolean {
            "True".into()
        } else {
            "False".into()
        }
    } else {
        vulse_model::text_value(value)
    }
}
fn guided_draft(formula: &str, fields: &[FieldDef]) -> Option<Draft> {
    let draft = Draft::from_formula(formula)?;
    // Only offer guided mode when regenerating this expression preserves its meaning.
    if vulse_model::inspect_formula(&draft.formula(fields).ok()?).ok()?
        == vulse_model::inspect_formula(formula).ok()?
    {
        Some(draft)
    } else {
        None
    }
}

pub(crate) fn type_label(f: &FieldDef) -> &'static str {
    match f.kind {
        FieldType::Number => "Number",
        FieldType::String => "Text",
        FieldType::Boolean => "True / false",
        FieldType::Date => "Date",
        FieldType::Calculated => "Calculated",
        FieldType::Array => "List",
        FieldType::Json => "JSON",
        FieldType::PortableText => "Rich text",
        FieldType::Asset => "Asset",
        FieldType::Reference => "Reference",
    }
}
/// A function's description and example, for help while typing.
pub(crate) fn describe(name: &str) -> (&'static str, &'static str) {
    let h = help(name);
    (h.description, h.example)
}

/// What argument `index` (from 0) of `name` expects.
pub(crate) fn argument_hint(name: &str, index: usize) -> Option<String> {
    let h = help(name);
    // Criteria functions repeat (list, condition) pairs after their values.
    let position = match name {
        "SUMIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS" if index > 0 => 1 + (index - 1) % 2,
        "COUNTIFS" => index % 2,
        _ if h.variadic => index.min(h.args.len() - 1),
        _ => index,
    };
    let argument = h.args.get(position)?;
    let label = match (name, position, index) {
        ("SUMIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS", 1, _) | ("COUNTIFS", 0, _) => "List to check",
        ("SUMIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS", 2, _) | ("COUNTIFS", 1, _) => "Condition for that list",
        _ => argument.label,
    };
    Some(format!(
        "{label}{} — {}",
        if argument.optional { " (optional)" } else { "" },
        kind_hint(argument.kind)
    ))
}

fn kind_hint(kind: Kind) -> &'static str {
    match kind {
        Kind::Number => "a numeric field or a fixed number",
        Kind::Text => "a text field or fixed text",
        Kind::Date => "YYYY-MM-DD text or an Excel serial number",
        Kind::Boolean => "a true / false value",
        Kind::Condition => "a boolean field or a comparison",
        Kind::Values => "numbers or a numeric list",
        Kind::Array => "a list field",
        Kind::Any => "a field or a fixed value",
    }
}
fn compatible(kind: Kind, f: &FieldDef) -> bool {
    if f.kind == FieldType::Calculated {
        return true;
    }
    match kind {
        Kind::Number => f.kind == FieldType::Number,
        Kind::Text => f.kind == FieldType::String,
        Kind::Date => matches!(
            f.kind,
            FieldType::Date | FieldType::String | FieldType::Number
        ),
        Kind::Boolean => f.kind == FieldType::Boolean,
        Kind::Values => {
            f.kind == FieldType::Number
                || (f.kind == FieldType::Array && f.items == Some(ArrayItems::Number))
        }
        Kind::Array => matches!(f.kind, FieldType::Array | FieldType::Json),
        Kind::Condition => matches!(
            f.kind,
            FieldType::Number | FieldType::Boolean | FieldType::String
        ),
        Kind::Any => true,
    }
}
#[derive(Clone, PartialEq, Debug)]
struct Draft {
    task: String,
    inputs: Vec<Binding>,
}
impl Draft {
    fn arguments(&self) -> Vec<Argument> {
        if self.task == "multiply" {
            return vec![
                arg("Value to multiply", Kind::Number),
                arg("Multiply by", Kind::Number),
            ];
        }
        if self.task == "percentage" {
            return vec![arg("Part", Kind::Number), arg("Total", Kind::Number)];
        }
        let h = help(&self.task);
        (0..self.inputs.len())
            .map(|i| h.args.get(i).unwrap_or(h.args.last().unwrap()).clone())
            .collect()
    }
    fn new(task: &str) -> Self {
        if task == "multiply" {
            return Self {
                task: task.into(),
                inputs: vec![Binding::empty(), Binding::fixed(Kind::Number, "2")],
            };
        }
        if task == "percentage" {
            return Self {
                task: task.into(),
                inputs: vec![Binding::empty(), Binding::empty()],
            };
        }
        let h = help(task);
        let mut args = h.args.clone();
        if h.variadic {
            args.push(h.args.last().unwrap().clone());
        }
        let inputs = args
            .iter()
            .filter(|a| !a.optional)
            .map(|a| {
                a.default
                    .map(|v| Binding::fixed(a.kind, v))
                    .unwrap_or_else(Binding::empty)
            })
            .collect();
        Self {
            task: task.into(),
            inputs,
        }
    }
    fn formula(&self, fields: &[FieldDef]) -> Result<String, String> {
        let values = self
            .inputs
            .iter()
            .zip(self.arguments())
            .map(|(b, a)| b.expression(&a, fields))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(if self.task == "multiply" {
            format!("={} * {}", values[0], values[1])
        } else if self.task == "percentage" {
            format!("=({} / {}) * 100", values[0], values[1])
        } else {
            format!("={}({})", self.task, values.join(", "))
        })
    }
    fn from_formula(formula: &str) -> Option<Self> {
        let e = vulse_model::inspect_formula(formula).ok()?;
        let simple = |value: &Expr| {
            if matches!(value, Expr::Binary(..)) {
                None
            } else {
                Binding::from_expr(value)
            }
        };
        match &e {
            Expr::Binary(op, a, b) if op == "*" => {
                if matches!(b.as_ref(),Expr::Literal(v) if v.as_f64()==Some(100.0))
                    && let Expr::Binary(op, part, total) = a.as_ref()
                        && op == "/" {
                            return Some(Self {
                                task: "percentage".into(),
                                inputs: vec![simple(part)?, simple(total)?],
                            });
                        }
                Some(Self {
                    task: "multiply".into(),
                    inputs: vec![simple(a)?, simple(b)?],
                })
            }
            Expr::Call(name, args) => Some(Self {
                task: name.clone(),
                inputs: args
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        let h = help(name);
                        let kind = h.args.get(index).unwrap_or(h.args.last().unwrap()).kind;
                        if kind == Kind::Condition {
                            Binding::from_expr(value)
                        } else {
                            simple(value)
                        }
                    })
                    .collect::<Option<Vec<_>>>()?,
            }),
            _ => None,
        }
    }
}
fn task_title(task: &str) -> String {
    match task {
        "multiply" => "Multiply values".into(),
        "percentage" => "Calculate a percentage".into(),
        "SUM" => "Add values".into(),
        "TEXTJOIN" => "Combine text".into(),
        "IF" => "If / otherwise".into(),
        "ROUND" => "Round a number".into(),
        _ => format!("{task} calculation"),
    }
}

#[derive(Properties, PartialEq)]
struct InputProps {
    argument: ArgumentView,
    index: usize,
    binding: Binding,
    fields: Vec<FieldDef>,
    on_change: Callback<Binding>,
}
#[derive(Clone, PartialEq)]
struct ArgumentView {
    label: String,
    kind: Kind,
}
#[function_component(CalculationInput)]
fn calculation_input(props: &InputProps) -> Html {
    let binding = &props.binding;
    let kind = props.argument.kind;
    let label = format!("{} {}", props.argument.label, props.index + 1);
    let selected_field = binding
        .source
        .strip_prefix("field:")
        .and_then(|name| props.fields.iter().find(|f| f.name == name));
    let comparison = kind == Kind::Condition
        && selected_field
            .is_some_and(|f| f.kind != FieldType::Boolean || binding.comparison != "truthy");
    let comparison_text = selected_field.is_some_and(|f| f.kind == FieldType::String)
        || serde_json::from_str::<Value>(&binding.compare_value).is_ok_and(|v| v.is_string());
    let comparison_boolean = selected_field.is_some_and(|f| f.kind == FieldType::Boolean);
    let fixed_sources: Vec<(&str, &str)> = match kind {
        Kind::Number | Kind::Values => vec![("number", "Fixed number")],
        Kind::Text => vec![("text", "Fixed text")],
        Kind::Date => vec![
            ("text", "Fixed date text"),
            ("number", "Excel serial number"),
        ],
        Kind::Boolean | Kind::Condition => vec![("boolean", "True / false")],
        Kind::Array => vec![],
        Kind::Any => vec![
            ("text", "Fixed text"),
            ("number", "Fixed number"),
            ("boolean", "True / false"),
        ],
    };
    html! {<div class="calculation-input"><label>{props.argument.label.clone()}<select aria-label={format!("{label} source")} value={binding.source.clone()} onchange={Callback::from({let change=props.on_change.clone();let binding=binding.clone();let fields=props.fields.clone();move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();let mut next=binding.clone();next.source=select.value();if next.source=="boolean"{next.value="true".into();}
                if let Some(f)=next.source.strip_prefix("field:").and_then(|n|fields.iter().find(|f|f.name==n)){next.comparison=if f.kind==FieldType::String{"="}else if f.kind==FieldType::Boolean{"truthy"}else{">"}.into();next.compare_value=if f.kind==FieldType::String{json!("Example").to_string()}else{"0".into()};}change.emit(next);}})}>
        <option value="" selected={binding.source.is_empty()}>{"Choose a field or fixed value…"}</option><optgroup label="Collection fields">{for props.fields.iter().filter(|f|compatible(kind,f)).map(|f|html!{<option value={format!("field:{}",f.name)} selected={binding.source==format!("field:{}",f.name)}>{format!("{} [{}] · {}",f.display_label(),f.name,type_label(f))}</option>})}</optgroup><optgroup label="Fixed values">{for fixed_sources.into_iter().map(|(value,text)|html!{<option {value} selected={binding.source==value}>{text}</option>})}</optgroup></select></label>
        if !binding.source.is_empty() && !binding.source.starts_with("field:") {
            if binding.source=="boolean" {<label class="calculation-fixed"><span class="muted">{"Fixed value"}</span><select aria-label={format!("{label} fixed value")} value={binding.value.clone()} onchange={Callback::from({let change=props.on_change.clone();let binding=binding.clone();move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();let mut next=binding.clone();next.value=select.value();change.emit(next);}})}><option value="true" selected={binding.value=="true"}>{"True"}</option><option value="false" selected={binding.value=="false"}>{"False"}</option></select></label>}
            else {<label class="calculation-fixed"><span class="muted">{"Fixed value"}</span><input aria-label={format!("{label} fixed value")} type={if binding.source=="number"{"number"}else{"text"}} step="any" placeholder={if binding.source=="json"{"[1, 2, 3]"}else if kind==Kind::Date{"YYYY-MM-DD"}else{"Enter a value"}} value={binding.value.clone()} oninput={Callback::from({let change=props.on_change.clone();let binding=binding.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();let mut next=binding.clone();next.value=input.value();change.emit(next);}})}/></label>}
        }
        if comparison {<div class="condition-controls"><label>{"Compare"}<select aria-label={format!("{label} comparison")} value={binding.comparison.clone()} onchange={Callback::from({let change=props.on_change.clone();let binding=binding.clone();move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();let mut next=binding.clone();next.comparison=select.value();change.emit(next);}})}>{for [("truthy","Is true / nonzero"),("=","Equals"),("<>","Does not equal"),(">","Greater than"),("<","Less than"),(">=","At least"),("<=","At most")].into_iter().map(|(value,text)|html!{<option {value} selected={binding.comparison==value}>{text}</option>})}</select></label>if binding.comparison!="truthy"{<label>{"Comparison value"}<input aria-label={format!("{label} comparison value")} type={if comparison_text || comparison_boolean{"text"}else{"number"}} step="any" value={if comparison_text{serde_json::from_str::<Value>(&binding.compare_value).ok().and_then(|v|v.as_str().map(str::to_string)).unwrap_or_default()}else{binding.compare_value.clone()}} oninput={Callback::from({let change=props.on_change.clone();let binding=binding.clone();let text=comparison_text;move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();let mut next=binding.clone();next.compare_value=if text{json!(input.value()).to_string()}else{input.value()};change.emit(next);}})}/></label>}</div>}
    </div>}
}

#[derive(Properties, PartialEq)]
pub struct FormulaEditorProps {
    pub field: FieldDef,
    pub fields: Vec<FieldDef>,
    pub records: Vec<DocumentView>,
    pub on_change: Callback<FieldDef>,
    /// Other collections whose fields formulas can read.
    #[prop_or_default]
    pub collections: Vec<CollectionView>,
    /// This collection's name.
    #[prop_or_default]
    pub current: String,
}
#[derive(Clone, PartialEq)]
enum Mode {
    Guided(Draft),
    Lookup(LookupDraft),
    Advanced,
}
/// The editing mode that shows `formula` best.
fn mode_for(formula: &str, inputs: &[FieldDef]) -> Mode {
    if formula.is_empty() {
        Mode::Guided(Draft::new("multiply"))
    } else if let Some(draft) = guided_draft(formula, inputs) {
        Mode::Guided(draft)
    } else if let Some(draft) = LookupDraft::from_formula(formula) {
        Mode::Lookup(draft)
    } else {
        Mode::Advanced
    }
}
#[function_component(FormulaEditor)]
pub fn formula_editor(props: &FormulaEditorProps) -> Html {
    let formula = props.field.formula.clone().unwrap_or_default();
    let inputs: Vec<_> = props
        .fields
        .iter()
        .filter(|f| f.name != props.field.name)
        .cloned()
        .collect();
    let others: Vec<CollectionView> = props
        .collections
        .iter()
        .filter(|c| c.name != props.current)
        .cloned()
        .collect();
    let mode = use_state(|| mode_for(&formula, &inputs));
    let library = use_state(|| false);
    let textarea = use_node_ref();
    let selection = use_state(|| {
        (
            formula.encode_utf16().count(),
            formula.encode_utf16().count(),
        )
    });
    let last_emitted = use_mut_ref(|| formula.clone());
    // Reconcile external edits/reordering, while retaining incomplete named-input drafts.
    {
        let mode = mode.clone();
        let inputs = inputs.clone();
        let last_emitted = last_emitted.clone();
        use_effect_with(formula.clone(), move |formula| {
            if *last_emitted.borrow() != *formula {
                mode.set(mode_for(formula, &inputs));
                *last_emitted.borrow_mut() = formula.clone();
            }
        });
    }
    let emit = {
        let field = props.field.clone();
        let change = props.on_change.clone();
        let last_emitted = last_emitted.clone();
        Callback::from(move |formula: String| {
            *last_emitted.borrow_mut() = formula.clone();
            let mut field = field.clone();
            field.formula = Some(formula);
            field.required = false;
            change.emit(field);
        })
    };
    let set_draft = {
        let mode = mode.clone();
        let emit = emit.clone();
        let fields = props.fields.clone();
        Callback::from(move |draft: Draft| {
            emit.emit(draft.formula(&fields).unwrap_or_default());
            mode.set(Mode::Guided(draft));
        })
    };
    let set_lookup = {
        let mode = mode.clone();
        let emit = emit.clone();
        Callback::from(move |draft: LookupDraft| {
            emit.emit(draft.formula().unwrap_or_default());
            mode.set(Mode::Lookup(draft));
        })
    };

    let draft_error = match &*mode {
        Mode::Guided(draft) => draft.formula(&inputs).err(),
        Mode::Lookup(draft) => draft.formula().err(),
        Mode::Advanced => None,
    };
    let guided_available = !matches!(mode_for(&formula, &inputs), Mode::Advanced);
    let task = match &*mode {
        Mode::Guided(draft) => draft.task.clone(),
        Mode::Lookup(_) => "lookup".to_string(),
        Mode::Advanced => String::new(),
    };
    let task_select = html! {<div class="calculation-task"><label>{"Calculation"}<select aria-label="Calculation" value={task.clone()} onchange={Callback::from({let set_draft=set_draft.clone();let set_lookup=set_lookup.clone();let others=others.clone();let library=library.clone();let task=task.clone();move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();let value=select.value();if value=="more"{library.set(true);select.set_value(&task);}else if value=="lookup"{if let Some(first)=others.first(){set_lookup.emit(LookupDraft::new(first));}library.set(false);}else{set_draft.emit(Draft::new(&value));library.set(false);}}})}>{for [("multiply","Multiply values"),("SUM","Add values"),("percentage","Calculate a percentage"),("TEXTJOIN","Combine text"),("IF","If / otherwise"),("ROUND","Round a number")].into_iter().map(|(value,label)|html!{<option {value} selected={task==value}>{label}</option>})}if !others.is_empty(){<option value="lookup" selected={task=="lookup"}>{"From another collection"}</option>}if !["multiply","SUM","percentage","TEXTJOIN","IF","ROUND","lookup"].contains(&task.as_str()){<option value={task.clone()} selected={true}>{task_title(&task)}</option>}<option value="more">{"More calculations…"}</option></select></label>
        <p class="muted">{match task.as_str(){"multiply"=>"Multiply a field by another field or a fixed number.","percentage"=>"Divide the part by the total, then multiply by 100. A result of 25 means 25%.","lookup"=>"Sum, count or look up values in another collection, using the records that match this record.","" => "",name=>help(name).description}}</p></div>};
    html! {<section class="formula-editor" aria-label={format!("{} calculation",props.field.name)}>
        <div class="calculation-heading"><div><h3>{"How should this field be calculated?"}</h3><p class="muted">{"Read-only · Updates automatically when its input fields change."}</p></div><button type="button" class="secondary" onclick={Callback::from({let mode=mode.clone();let formula=formula.clone();let set_draft=set_draft.clone();let inputs=inputs.clone();move |_|{match &*mode{Mode::Guided(_)|Mode::Lookup(_)=>mode.set(Mode::Advanced),Mode::Advanced=>match mode_for(&formula,&inputs){Mode::Advanced=>set_draft.emit(Draft::new("multiply")),next=>mode.set(next)}}}})}>{if !matches!(&*mode,Mode::Advanced){"Edit formula"}else if guided_available{"Use guided builder"}else{"Start a new guided calculation"}}</button></div>
        if let Mode::Lookup(draft)=&*mode {
            {task_select.clone()}
            <LookupBuilder draft={draft.clone()} collections={others.clone()} inputs={inputs.clone()} on_change={set_lookup.clone()}/>
            <div class="calculation-formula"><span class="muted">{"Formula"}</span><code>{if formula.is_empty(){"Choose your inputs to build the formula"}else{&formula}}</code></div>
        }
        if let Mode::Guided(draft)=&*mode {
            {task_select}
            <div class="calculation-inputs">{for draft.inputs.iter().zip(draft.arguments()).enumerate().map(|(index,(binding,argument))|{let change={let draft=draft.clone();let set_draft=set_draft.clone();Callback::from(move |binding:Binding|{let mut draft=draft.clone();draft.inputs[index]=binding;set_draft.emit(draft);})};html!{<div class="calculation-input-wrap"><CalculationInput key={index} index={index} argument={ArgumentView{label:argument.label.into(),kind:argument.kind}} binding={binding.clone()} fields={inputs.clone()} on_change={change}/>
            if draft.inputs.len()>1 && !["multiply","percentage"].contains(&draft.task.as_str()) && ((help(&draft.task).variadic && index>=help(&draft.task).args.len()) || argument.optional) {<button class="text-button" type="button" aria-label={format!("Remove input {}",index+1)} onclick={Callback::from({let draft=draft.clone();let set_draft=set_draft.clone();move |_|{let mut draft=draft.clone();draft.inputs.remove(index);set_draft.emit(draft);}})}>{"Remove input"}</button>}
            </div>}})}</div>
            if !["multiply","percentage"].contains(&draft.task.as_str()) && (help(&draft.task).variadic || draft.inputs.len()<help(&draft.task).args.len()) {<button type="button" class="secondary" onclick={Callback::from({let draft=draft.clone();let set_draft=set_draft.clone();move |_|{let mut draft=draft.clone();let h=help(&draft.task);let arg=h.args.get(draft.inputs.len()).unwrap_or(h.args.last().unwrap());draft.inputs.push(arg.default.map(|v|Binding::fixed(arg.kind,v)).unwrap_or_else(Binding::empty));set_draft.emit(draft);}})}>{if help(&draft.task).variadic{"+ Add another value"}else{"+ Add optional argument"}}</button>}
            <div class="calculation-formula"><span class="muted">{"Formula"}</span><code>{if formula.is_empty(){"Choose your inputs to build the formula"}else{&formula}}</code></div>
        }
        if matches!(&*mode,Mode::Advanced) {<AdvancedFormula suggestions={formula_suggestions(&inputs,&props.collections,&props.current)} textarea={textarea.clone()} on_selection={Callback::from({let selection=selection.clone();move |next|selection.set(next)})} value={formula.clone()} fields={inputs.clone()} on_change={emit.clone()} on_library={Callback::from({let library=library.clone();move |_|library.set(!*library)})}/>}
        if *library {<FunctionLibrary advanced={matches!(&*mode,Mode::Advanced)} on_use={Callback::from({let set_draft=set_draft.clone();let library=library.clone();move |name:String|{set_draft.emit(Draft::new(&name));library.set(false);}})} on_insert={Callback::from({let emit=emit.clone();let formula=formula.clone();let selection=selection.clone();let textarea=textarea.clone();move |name:String|{let (next,caret)=insert_function(&formula,*selection,&name);if let Some(area)=textarea.cast::<HtmlTextAreaElement>(){area.set_value(&next);area.focus().ok();area.set_selection_range(caret as u32,caret as u32).ok();}selection.set((caret,caret));emit.emit(next);}})}/>}
        <CalculationPreview field={props.field.clone()} fields={props.fields.clone()} records={props.records.clone()} draft_error={draft_error}/>
    </section>}
}

#[derive(Properties, PartialEq)]
pub(crate) struct LibraryProps {
    pub advanced: bool,
    /// Offers replacing the formula with a guided calculation.
    #[prop_or(true)]
    pub replace: bool,
    pub on_use: Callback<String>,
    pub on_insert: Callback<String>,
}
#[function_component(FunctionLibrary)]
pub(crate) fn function_library(props: &LibraryProps) -> Html {
    let query = use_state(String::new);
    let category = use_state(|| "All".to_string());
    let selected = use_state(|| None::<String>);
    let functions: Vec<_> = vulse_model::FUNCTIONS
        .iter()
        .filter(|(name, usage)| {
            let h = help(name);
            let searchable =
                format!("{name} {usage} {} {}", h.description, h.example).to_lowercase();
            (*category == "All" || *category == h.category)
                && query
                    .to_lowercase()
                    .split_whitespace()
                    .all(|word| searchable.contains(word))
        })
        .collect();
    html! {<div class="function-library"><h4>{"Find a calculation"}</h4><p class="muted">{"Search by what you want to do. Select a function to see its inputs and an example."}</p><div class="function-search"><input type="search" aria-label="Search functions" placeholder="Search functions, e.g. round or join…" value={(*query).clone()} oninput={Callback::from({let query=query.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();query.set(input.value());}})}/><select aria-label="Function category" value={(*category).clone()} onchange={Callback::from({let category=category.clone();move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();category.set(select.value());}})}>{for ["All","Numbers","Text","Logic","Dates","Lists"].into_iter().map(|name|html!{<option value={name} selected={*category==name}>{name}</option>})}</select></div>
    <div class="function-library-grid"><div class="function-results" aria-label="Matching functions">{for functions.iter().map(|(name,_)|{let h=help(name);html!{<button type="button" class={if selected.as_deref()==Some(*name){"function-choice active"}else{"function-choice"}} aria-pressed={(selected.as_deref()==Some(*name)).to_string()} onclick={Callback::from({let selected=selected.clone();let name=name.to_string();move |_|selected.set(Some(name.clone()))})}><strong>{*name}</strong><span>{h.description}</span></button>}})}if functions.is_empty(){<p class="muted">{"No functions found. Try another search."}</p>}</div>
    <div class="function-detail">if let Some(name)=selected.as_ref(){<h4>{name.clone()}</h4><p>{help(name).description}</p><code>{vulse_model::FUNCTIONS.iter().find(|(n,_)|*n==name).map(|(_,usage)|*usage).unwrap_or_default()}</code><ul>{for help(name).args.iter().map(|a|html!{<li><strong>{a.label}</strong>{if a.optional{" (optional)"}else{""}}{format!(" — {}",kind_hint(a.kind))}</li>})}</ul><p class="muted">{"Example"}</p><code>{format!("={}",help(name).example)}</code><div class="row">
    if props.advanced {<button type="button" onclick={Callback::from({let insert=props.on_insert.clone();let name=name.clone();move |_|insert.emit(name.clone())})}>{"Insert function at cursor"}</button>}
    if props.replace {<button type="button" class="secondary" onclick={Callback::from({let change=props.on_use.clone();let name=name.clone();move |_|change.emit(name.clone())})}>{if props.advanced{format!("Replace calculation with {name}")}else{format!("Use {name}")}}</button>}</div>if props.replace {<p class="muted">{if props.advanced{"Insert keeps the surrounding formula. Replace starts a new guided calculation."}else{"Use starts a new calculation with named input controls."}}</p>}}
    else {<p class="muted">{"Choose a function to see how it works."}</p>}</div></div></div>}
}

pub(crate) fn replace_chars(text: &str, start: usize, end: usize, insert: &str) -> String {
    format!(
        "{}{}{}",
        text.chars().take(start).collect::<String>(),
        insert,
        text.chars().skip(end).collect::<String>()
    )
}
pub(crate) fn insert_function(
    formula: &str,
    selection: (usize, usize),
    name: &str,
) -> (String, usize) {
    let start = vulse_model::utf16_to_char_index(formula, selection.0).min(formula.chars().count());
    let end = vulse_model::utf16_to_char_index(formula, selection.1)
        .max(start)
        .min(formula.chars().count());
    let mut selected = formula
        .chars()
        .skip(start)
        .take(end - start)
        .collect::<String>();
    let selected_equals = start == 0 && selected.starts_with('=');
    if selected_equals {
        selected.remove(0);
    }
    let h = help(name);
    let args: Vec<String> = h
        .args
        .iter()
        .filter(|a| !a.optional)
        .enumerate()
        .map(|(i, a)| {
            if i == 0 && !selected.is_empty() {
                selected.clone()
            } else {
                a.default
                    .map(|v| {
                        Binding::fixed(a.kind, v)
                            .expression(a, &[])
                            .unwrap_or_default()
                    })
                    .unwrap_or_default()
            }
        })
        .collect();
    let snippet = format!("{name}({})", args.join(", "));
    let prefix = if formula.is_empty() || selected_equals {
        "="
    } else {
        ""
    };
    let next = replace_chars(formula, start, end, &format!("{prefix}{snippet}"));
    let caret = next
        .chars()
        .take(start + prefix.len() + name.len() + 1)
        .map(char::len_utf16)
        .sum();
    (next, caret)
}
#[derive(Properties, PartialEq)]
struct AdvancedProps {
    value: String,
    fields: Vec<FieldDef>,
    suggestions: Vec<Suggestion>,
    on_change: Callback<String>,
    on_library: Callback<()>,
    on_selection: Callback<(usize, usize)>,
    textarea: NodeRef,
}
#[function_component(AdvancedFormula)]
fn advanced_formula(props: &AdvancedProps) -> Html {
    html! {<div class="advanced-formula"><FormulaInput lookups={true} textarea={props.textarea.clone()} value={props.value.clone()} suggestions={props.suggestions.clone()} on_change={props.on_change.clone()} on_selection={props.on_selection.clone()} placeholder="=ROUND([price] * [quantity], 2)"/>
        <p class="muted">{"Use [field_name] for a field, [collection.field] for a field of another collection, quoted text for a string, and + − * / for arithmetic. Type a function name or [ for suggestions, then press Enter to insert. Select text to wrap it in a function."}</p><div class="row"><button type="button" class="secondary" onclick={Callback::from({let change=props.on_library.clone();move |_|change.emit(())})}>{"Browse functions"}</button><label class="advanced-insert-field">{"Insert field"}<select aria-label="Insert input field" value="" onchange={Callback::from({let change=props.on_change.clone();let value=props.value.clone();let node=props.textarea.clone();move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();if select.value().is_empty(){return;}let name=select.value();let (start,end)=node.cast::<HtmlTextAreaElement>().map(|a|(a.selection_start().ok().flatten().unwrap_or(0) as usize,a.selection_end().ok().flatten().unwrap_or(0) as usize)).unwrap_or((value.encode_utf16().count(),value.encode_utf16().count()));let at=vulse_model::utf16_to_char_index(&value,start);let end=vulse_model::utf16_to_char_index(&value,end);let prefix=if value.is_empty(){"="}else{""};change.emit(replace_chars(&value,at,end,&format!("{prefix}[{name}]")));select.set_value("");}})}><option value="" selected={true}>{"Choose a field…"}</option>{for props.fields.iter().map(|f|html!{<option value={f.name.clone()}>{format!("{} [{}] · {}",f.display_label(),f.name,type_label(f))}</option>})}</select></label></div>
    </div>}
}

/// Field names a formula can use: this collection's other fields, and the
/// fields of other collections as `collection.field`, except those that
/// read other collections themselves.
pub(crate) fn formula_suggestions(
    inputs: &[FieldDef],
    collections: &[CollectionView],
    current: &str,
) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = inputs
        .iter()
        .map(|f| Suggestion {
            name: f.name.clone(),
            label: f.display_label().to_string(),
            detail: type_label(f).to_string(),
        })
        .collect();
    for collection in collections.iter().filter(|c| c.name != current) {
        for f in &collection.blueprint.fields {
            let reads_collections = f
                .formula
                .as_deref()
                .is_some_and(|formula| !vulse_model::formula_lookups(formula).is_empty());
            if f.kind == FieldType::Calculated && reads_collections {
                continue;
            }
            out.push(Suggestion {
                name: format!("{}.{}", collection.name, f.name),
                label: f.display_label().to_string(),
                detail: format!("{} collection · {}", collection.name, type_label(f)),
            });
        }
    }
    out
}

fn preview_schema(field: &FieldDef, fields: &[FieldDef]) -> (Blueprint, Vec<FieldDef>) {
    let mut names = BTreeSet::new();
    let mut pending = vec![field.name.clone()];
    while let Some(name) = pending.pop() {
        if !names.insert(name.clone()) {
            continue;
        }
        if let Some(f) = fields.iter().find(|f| f.name == name)
            && let Some(formula) = &f.formula
                && let Ok(deps) = vulse_model::formula_dependencies(formula) {
                    pending.extend(deps);
                }
    }
    let schema = Blueprint {
        title_field: None,
        fields: fields
            .iter()
            .filter(|f| names.contains(&f.name))
            .cloned()
            .collect(),
    };
    let inputs = schema
        .fields
        .iter()
        .filter(|f| f.kind != FieldType::Calculated)
        .cloned()
        .collect();
    (schema, inputs)
}
fn sample_default(field: &FieldDef) -> String {
    match field.kind {
        FieldType::Number => "10".into(),
        FieldType::Boolean => "true".into(),
        FieldType::Date => "2026-01-15".into(),
        FieldType::Array => {
            if field.items == Some(ArrayItems::Number) {
                "[1, 2, 3]".into()
            } else {
                "[\"Example\", \"Other\"]".into()
            }
        }
        FieldType::Json => "[1, 2, 3]".into(),
        FieldType::PortableText => "[]".into(),
        _ => "Example".into(),
    }
}
fn sample_value(field: &FieldDef, value: &str) -> Result<Value, String> {
    Ok(match field.kind {
        FieldType::Number => {
            if value.is_empty() {
                Value::Null
            } else {
                serde_json::from_str::<Value>(value)
                    .ok()
                    .filter(Value::is_number)
                    .ok_or_else(|| format!("{}: enter a number for the preview.", field.name))?
            }
        }
        FieldType::Boolean => json!(value == "true"),
        FieldType::Json | FieldType::Array | FieldType::PortableText => serde_json::from_str(value)
            .map_err(|_| format!("{}: enter valid JSON for the preview.", field.name))?,
        _ => json!(value),
    })
}
fn friendly_error(message: &str) -> String {
    if message.contains("#DIV/0!") {
        return "Cannot divide by zero. Change the denominator or choose a fallback with IFERROR."
            .into();
    }
    if message.contains("expected number") {
        return "An input is not numeric. Check its field type and the preview value.".into();
    }
    if message.contains("expected array") {
        return "A list was expected. Choose a list field and check its preview value.".into();
    }
    if message.contains("circular") {
        return "This calculation refers back to itself. Choose different input fields to break the cycle.".into();
    }
    if message.contains("#N/A") {
        return "No matching value was found for these inputs.".into();
    }
    if message.contains("unknown field") {
        return format!("{message}. Select an existing field or correct its name.");
    }
    if message.contains("expected a value") {
        return "Finish the formula's inputs. Insert a field or enter a fixed value where one is missing.".into();
    }
    message.into()
}
#[derive(Properties, PartialEq)]
struct PreviewProps {
    field: FieldDef,
    fields: Vec<FieldDef>,
    records: Vec<DocumentView>,
    draft_error: Option<String>,
}
#[function_component(CalculationPreview)]
fn calculation_preview(props: &PreviewProps) -> Html {
    let source = use_state(String::new);
    let overrides = use_state(BTreeMap::<String, String>::new);
    if props.draft_error.is_none()
        && props
            .field
            .formula
            .as_deref()
            .is_some_and(|f| !vulse_model::formula_lookups(f).is_empty())
    {
        return html! {<div class="calculation-preview"><p class="muted" aria-label="Calculation preview result">{"This calculation reads another collection, so the server works it out. Save the fields to see the results in your records."}</p></div>};
    }
    let (schema, inputs) = preview_schema(&props.field, &props.fields);
    let record = props.records.iter().find(|r| r.id == *source);
    let samples: Vec<_> = inputs
        .iter()
        .map(|f| {
            let value = overrides.get(&f.name).cloned().unwrap_or_else(|| {
                if let Some(record) = record {
                    record
                        .body
                        .get(&f.name)
                        .map(|v| {
                            if let Some(text) = v.as_str() {
                                text.into()
                            } else if v.is_null() {
                                String::new()
                            } else {
                                v.to_string()
                            }
                        })
                        .unwrap_or_default()
                } else {
                    sample_default(f)
                }
            });
            (f.clone(), value)
        })
        .collect();
    let mut body = serde_json::Map::new();
    let mut sample_error = None;
    for (field, value) in &samples {
        let value = if let Some(record) = record.filter(|_| !overrides.contains_key(&field.name)) {
            Ok(record.body.get(&field.name).cloned().unwrap_or(Value::Null))
        } else {
            sample_value(field, value)
        };
        match value {
            Ok(value) => {
                body.insert(field.name.clone(), value);
            }
            Err(error) => sample_error = Some(error),
        }
    }
    let validation = vulse_model::validate_formulas(&schema).err();
    let preview_error = props
        .draft_error
        .clone()
        .or_else(|| validation.as_ref().map(|e| friendly_error(e)))
        .or(sample_error);
    let (calculated, errors) = if preview_error.is_none() {
        vulse_model::calculate_document(&schema, &Value::Object(body))
    } else {
        (Value::Null, BTreeMap::new())
    };
    let runtime_error = errors
        .get(&props.field.name)
        .or_else(|| errors.get("blueprint"))
        .map(|e| friendly_error(e));
    let result = calculated
        .get(&props.field.name)
        .cloned()
        .unwrap_or(Value::Null);
    html! {<div class="calculation-preview"><div class="preview-heading"><div><h4>{"Try your calculation"}</h4><p class="muted">{"Preview changes stay here. They never change a saved record."}</p></div><label>{"Preview values"}<select aria-label="Preview values" value={(*source).clone()} onchange={Callback::from({let source=source.clone();let overrides=overrides.clone();move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();source.set(select.value());overrides.set(BTreeMap::new());}})}><option value="" selected={source.is_empty()}>{"Sample values"}</option>{for props.records.iter().map(|r|html!{<option value={r.id.clone()} selected={*source==r.id}>{format!("Record {}{}",r.id.chars().take(8).collect::<String>(),r.body.get("title").or_else(||r.body.get("name")).and_then(Value::as_str).map(|s|format!(" · {s}")).unwrap_or_default())}</option>})}</select></label></div>
        <div class="preview-inputs">{for samples.iter().map(|(field,value)|{let change={let overrides=overrides.clone();let name=field.name.clone();Callback::from(move |value:String|{let mut next=(*overrides).clone();next.insert(name.clone(),value);overrides.set(next);})};html!{<label>{format!("{} · {}",field.display_label(),type_label(field))}
        if field.kind==FieldType::Boolean {<select aria-label={format!("Preview {}",field.name)} value={value.clone()} onchange={Callback::from(move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();change.emit(select.value());})}><option value="true" selected={value=="true"}>{"True"}</option><option value="false" selected={value=="false"}>{"False"}</option></select>}
        else {<input aria-label={format!("Preview {}",field.name)} type={match field.kind{FieldType::Number=>"number",FieldType::Date=>"date",_=>"text"}} step="any" value={value.clone()} oninput={Callback::from(move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();change.emit(input.value());})}/>}
        </label>}})}</div>
        if let Some(error)=preview_error {<p class="calculation-feedback incomplete" role="status">{error}</p>}
        else {<p class="syntax-status">{"✓ Formula syntax and field references are valid"}</p>
            if let Some(error)=runtime_error {<p class="error" role="status">{error}</p>}
            else {<div class="preview-result"><span>{format!("{} · Preview result",if props.field.name.is_empty(){"Calculated field"}else{&props.field.name})}</span><output aria-label="Calculation preview result">{preview_text(&result)}</output>{if props.field.formula.as_deref().is_some_and(|f|Draft::from_formula(f).is_some_and(|d|d.task=="percentage")){html!{<span class="muted">{"Percent (25 means 25%)"}</span>}}else{html!{}}}</div>}
        }
        if !inputs.is_empty(){<p class="preview-dependencies muted">{format!("Updates when {} changes.",inputs.iter().map(|f|f.name.clone()).collect::<Vec<_>>().join(", "))}</p>}
    </div>}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn field(name: &str, kind: FieldType, formula: Option<&str>) -> FieldDef {
        FieldDef {
            label: None,
            description: None,
            example: None,
            name: name.into(),
            kind,
            formula: formula.map(str::to_string),
            required: false,
            collection: None,
            items: None,
            renamed_from: None,
            options: None,
            renamed_options: None,
        }
    }
    #[test]
    fn recognizes_existing_calculations_without_rewriting_custom_expressions() {
        let d = Draft::from_formula("=[price]*2").unwrap();
        assert_eq!(d.task, "multiply");
        assert_eq!(
            d.formula(&[field("price", FieldType::Number, None)])
                .unwrap(),
            "=[price] * 2"
        );
        assert!(Draft::from_formula("=ROUND([price] * [quantity], 2)").is_none());
        assert_eq!(
            Draft::from_formula("=([part]/[total])*100").unwrap().task,
            "percentage"
        );
    }
    #[test]
    fn quoted_text_and_conditions_round_trip_through_the_shared_parser() {
        let values = [
            json!("say \"hello\" \\ path"),
            json!("line one\nline two"),
            json!(true),
            json!(-2),
        ];
        for value in values {
            let formula = format!("={}", formula_literal(&value));
            let bp = Blueprint {
                title_field: None,
                fields: vec![field("result", FieldType::Calculated, Some(&formula))],
            };
            let (body, errors) = vulse_model::calculate_document(&bp, &json!({}));
            assert!(errors.is_empty());
            if value.is_number() {
                assert_eq!(body["result"].as_f64(), value.as_f64());
            } else {
                assert_eq!(body["result"], value);
            }
        }
        let fields = vec![
            field("price", FieldType::Number, None),
            field("approved", FieldType::Boolean, None),
        ];
        for formula in [
            "=IF([price] > 100, \"say \"\"hi\"\"\", \"No\")",
            "=IF([approved] = FALSE, \"No\", \"Yes\")",
        ] {
            assert!(guided_draft(formula, &fields).is_some());
        }
        assert!(guided_draft("=SUM([price] > 0)", &fields).is_none());
        assert!(guided_draft("=TRUE * 2", &fields).is_none());
        assert!(guided_draft("=([price] > 0) / 2 * 100", &fields).is_none());
        assert_eq!(
            insert_function("=[price]*2", (0, 10), "ROUND").0,
            "=ROUND([price]*2, 2)"
        );
    }

    #[test]
    fn metadata_covers_every_function_and_defaults_have_valid_arity() {
        for (name, _) in vulse_model::FUNCTIONS {
            let h = help(name);
            assert!(!h.description.is_empty());
            let mut draft = Draft::new(name);
            let arguments = draft.arguments();
            for (binding, argument) in draft.inputs.iter_mut().zip(arguments) {
                if binding.source.is_empty() {
                    if argument.kind == Kind::Array {
                        binding.source = "field:values".into();
                        continue;
                    }
                    *binding = Binding::fixed(
                        argument.kind,
                        if matches!(argument.kind, Kind::Boolean | Kind::Condition) {
                            "true"
                        } else {
                            "1"
                        },
                    );
                }
            }
            let formula = draft
                .formula(&[field("values", FieldType::Json, None)])
                .unwrap();
            assert!(
                vulse_model::inspect_formula(&formula).is_ok(),
                "{name}: {formula}"
            );
        }
    }
    #[test]
    fn preview_recalculates_dependencies_and_ignores_unrelated_incomplete_fields() {
        let fields = vec![
            field("price", FieldType::Number, None),
            field("tax", FieldType::Calculated, Some("=[price]*0.2")),
            field("total", FieldType::Calculated, Some("=[price]+[tax]")),
            field("unfinished", FieldType::Calculated, Some("")),
        ];
        let (bp, inputs) = preview_schema(&fields[2], &fields);
        assert_eq!(inputs.len(), 1);
        assert!(vulse_model::validate_formulas(&bp).is_ok());
        let (out, errors) = vulse_model::calculate_document(&bp, &json!({"price":100,"tax":999}));
        assert!(errors.is_empty());
        assert_eq!(out["total"], 120.0);
    }
    #[test]
    fn insertion_wraps_selection_and_preserves_unicode_and_surrounding_formula() {
        let formula = "=\"😀\" & [price]";
        let (next, _) = insert_function(formula, (8, 15), "ROUND");
        assert_eq!(next, "=\"😀\" & ROUND([price], 2)");
    }
}
