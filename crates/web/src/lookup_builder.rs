//! Guided calculations that read another collection: a sum, average,
//! minimum, maximum or count of matching records, or a value from the first
//! matching record. Supports math on the result and an empty result when
//! nothing matches. Builds these formulas and reads them back losslessly.

use serde_json::Value;
use vulse_model::{CollectionView, FieldDef, FieldType, FormulaExpression as Expr};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

pub(crate) const OPERATIONS: [(&str, &str); 6] = [
    ("SUMIFS", "Sum"),
    ("AVERAGEIFS", "Average"),
    ("MINIFS", "Smallest value"),
    ("MAXIFS", "Largest value"),
    ("COUNTIFS", "Count matching records"),
    ("XLOOKUP", "Value from the first matching record"),
];

const RESULT_OPERATIONS: &[(&str, &str)] = &[
    ("*", "Multiply by"),
    ("/", "Divide by"),
    ("+", "Add"),
    ("-", "Subtract"),
    ("^", "Raise to a power"),
    ("ROUND", "Round to decimal places"),
    ("ROUNDUP", "Round away from zero"),
    ("ROUNDDOWN", "Round towards zero"),
];

/// Applied in order to the result, before handling records with no match.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ResultStep {
    pub op: String,
    /// `fixed` for a number, or `field:<name>` for a field on this record.
    pub source: String,
    pub value: String,
}
impl Default for ResultStep {
    fn default() -> Self {
        Self {
            op: "*".into(),
            source: "fixed".into(),
            value: "2".into(),
        }
    }
}
impl ResultStep {
    fn operand(&self) -> Result<String, String> {
        if let Some(name) = self
            .source
            .strip_prefix("field:")
            .filter(|name| !name.is_empty())
        {
            return Ok(format!("[{name}]"));
        }
        if self.source == "fixed" {
            if let Ok(number) = self.value.trim().parse::<f64>() {
                if number.is_finite() {
                    return Ok(number.to_string());
                }
            }
        }
        Err("choose a numeric field or enter a number.".into())
    }
    fn apply(&self, expression: &str) -> Result<String, String> {
        let value = self.operand()?;
        match self.op.as_str() {
            "+" | "-" | "*" | "/" | "^" => Ok(format!("({expression}) {} {value}", self.op)),
            "ROUND" | "ROUNDUP" | "ROUNDDOWN" => Ok(format!("{}({expression}, {value})", self.op)),
            _ => Err("choose a math operation.".into()),
        }
    }
    fn from_expr(op: &str, value: &Expr) -> Option<Self> {
        let (source, value) = match value {
            Expr::Field(name) if !name.contains('.') => (format!("field:{name}"), String::new()),
            Expr::Literal(v) if v.is_number() => ("fixed".into(), v.as_f64()?.to_string()),
            Expr::Unary('-', inner) => {
                let Expr::Literal(v) = inner.as_ref() else {
                    return None;
                };
                ("fixed".into(), (-v.as_f64()?).to_string())
            }
            _ => return None,
        };
        Some(Self {
            op: op.into(),
            source,
            value,
        })
    }
}

/// "Their `field` equals this record's `source` (plus `adjust`)", or a fixed value.
#[derive(Clone, PartialEq, Debug, Default)]
pub(crate) struct Condition {
    /// A field of the other collection.
    pub field: String,
    /// `field:<name>` for this record's field, or `fixed`.
    pub source: String,
    /// The fixed value, or a number added to this record's field.
    pub value: String,
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) struct LookupDraft {
    pub collection: String,
    pub op: String,
    /// The field to sum, average or return; unused when counting.
    pub field: String,
    pub conditions: Vec<Condition>,
    /// XLOOKUP's result when nothing matches; empty for an error.
    pub fallback: String,
    /// Guard the whole calculation, including math, with a matching-record count.
    pub empty_when_missing: bool,
    pub steps: Vec<ResultStep>,
}

/// Fields of another collection a formula may read: not calculated fields
/// that read other collections themselves.
pub(crate) fn readable(collection: &CollectionView) -> Vec<&FieldDef> {
    collection
        .blueprint
        .fields
        .iter()
        .filter(|f| {
            f.kind != FieldType::Calculated
                || f.formula
                    .as_deref()
                    .is_none_or(|formula| vulse_model::formula_lookups(formula).is_empty())
        })
        .collect()
}

fn numeric(f: &FieldDef) -> bool {
    matches!(f.kind, FieldType::Number | FieldType::Calculated)
}

fn literal(text: &str) -> String {
    match text.trim().parse::<f64>() {
        Ok(n) if n.is_finite() => n.to_string(),
        _ => format!("\"{}\"", text.replace('"', "\"\"")),
    }
}

impl LookupDraft {
    pub fn new(collection: &CollectionView) -> Self {
        let fields = readable(collection);
        Self {
            collection: collection.name.clone(),
            op: "SUMIFS".into(),
            field: fields
                .iter()
                .find(|f| numeric(f))
                .map(|f| f.name.clone())
                .unwrap_or_default(),
            conditions: vec![Condition {
                field: fields.first().map(|f| f.name.clone()).unwrap_or_default(),
                ..Condition::default()
            }],
            fallback: String::new(),
            empty_when_missing: false,
            steps: Vec::new(),
        }
    }

    fn column(&self, field: &str) -> String {
        format!("[{}.{field}]", self.collection)
    }

    fn value(&self, condition: &Condition, index: usize) -> Result<String, String> {
        let number = || {
            condition
                .value
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
        };
        if let Some(name) = condition.source.strip_prefix("field:") {
            let adjust = if condition.value.trim().is_empty() {
                0.0
            } else {
                number().ok_or_else(|| {
                    format!("Condition {}: the adjustment must be a number.", index + 1)
                })?
            };
            return Ok(if adjust > 0.0 {
                format!("[{name}] + {adjust}")
            } else if adjust < 0.0 {
                format!("[{name}] - {}", -adjust)
            } else {
                format!("[{name}]")
            });
        }
        if condition.source == "fixed" && !condition.value.trim().is_empty() {
            return Ok(literal(&condition.value));
        }
        Err(format!(
            "Condition {}: choose a field of this record or enter a value.",
            index + 1
        ))
    }

    pub fn formula(&self) -> Result<String, String> {
        let base = self.base_formula()?;
        let mut expression = base.trim_start_matches('=').to_string();
        for (index, step) in self.steps.iter().enumerate() {
            expression = step
                .apply(&expression)
                .map_err(|error| format!("Math step {}: {error}", index + 1))?;
        }
        if self.empty_when_missing {
            let missing = if self.op == "XLOOKUP" {
                // XLOOKUP matches literal keys; COUNTIFS interprets wildcards and
                // comparison prefixes. Probe the matching key with XLOOKUP itself.
                // Its text length is always >= 0, including empty/null keys.
                let key = Self {
                    field: self.conditions[0].field.clone(),
                    fallback: String::new(),
                    ..self.clone()
                }
                .base_formula()?;
                format!("IFERROR(LEN({}), -1) = -1", key.trim_start_matches('='))
            } else {
                let count = Self {
                    op: "COUNTIFS".into(),
                    ..self.clone()
                }
                .base_formula()?;
                format!("{} = 0", count.trim_start_matches('='))
            };
            expression = format!("IF({missing}, \"\", {expression})");
        }
        Ok(format!("={expression}"))
    }

    fn base_formula(&self) -> Result<String, String> {
        if self.collection.is_empty() {
            return Err("Choose a collection.".into());
        }
        if self.op != "COUNTIFS" && self.field.is_empty() {
            return Err("Choose the field to calculate.".into());
        }
        if self.conditions.is_empty() {
            return Err("Add a condition for which records match.".into());
        }
        let mut pairs = Vec::new();
        for (index, condition) in self.conditions.iter().enumerate() {
            if condition.field.is_empty() {
                return Err(format!("Condition {}: choose a field to match.", index + 1));
            }
            pairs.push((self.column(&condition.field), self.value(condition, index)?));
        }
        if self.op == "XLOOKUP" {
            if pairs.len() != 1 {
                return Err(
                    "Looking up the first matching record requires one matching field.".into(),
                );
            }
            let (column, value) = &pairs[0];
            let fallback = if self.fallback.trim().is_empty() {
                String::new()
            } else {
                format!(", {}", literal(&self.fallback))
            };
            return Ok(format!(
                "=XLOOKUP({value}, {column}, {}{fallback})",
                self.column(&self.field)
            ));
        }
        let mut args = Vec::new();
        if self.op != "COUNTIFS" {
            args.push(self.column(&self.field));
        }
        for (column, value) in pairs {
            args.push(column);
            args.push(value);
        }
        Ok(format!("={}({})", self.op, args.join(", ")))
    }

    /// Reads back a formula this builder wrote, or `None`.
    pub fn from_formula(formula: &str) -> Option<Self> {
        let parsed = vulse_model::inspect_formula(formula).ok()?;
        let mut expression = &parsed;
        let mut empty_when_missing = false;
        if let Expr::Call(name, args) = expression {
            if name == "IF"
                && args.len() == 3
                && matches!(&args[1], Expr::Literal(Value::String(s)) if s.is_empty())
            {
                empty_when_missing = true;
                expression = &args[2];
            }
        }
        let mut steps = Vec::new();
        loop {
            match expression {
                Expr::Binary(op, left, right)
                    if ["+", "-", "*", "/", "^"].contains(&op.as_str()) =>
                {
                    steps.push(ResultStep::from_expr(op, right)?);
                    expression = left;
                }
                Expr::Call(op, args)
                    if ["ROUND", "ROUNDUP", "ROUNDDOWN"].contains(&op.as_str())
                        && args.len() == 2 =>
                {
                    steps.push(ResultStep::from_expr(op, &args[1])?);
                    expression = &args[0];
                }
                _ => break,
            }
        }
        steps.reverse();
        let Expr::Call(op, args) = expression else {
            return None;
        };
        let column = |e: &Expr| match e {
            Expr::Field(name) => name
                .split_once('.')
                .map(|(c, f)| (c.to_string(), f.to_string())),
            _ => None,
        };
        let condition = |field: String, e: &Expr| -> Option<Condition> {
            let (source, value) = match e {
                Expr::Field(name) if !name.contains('.') => {
                    (format!("field:{name}"), String::new())
                }
                Expr::Binary(sign, left, right) if sign == "+" || sign == "-" => {
                    let (Expr::Field(name), Expr::Literal(n)) = (left.as_ref(), right.as_ref())
                    else {
                        return None;
                    };
                    let n = n.as_f64()?;
                    if name.contains('.') {
                        return None;
                    }
                    (
                        format!("field:{name}"),
                        (if sign == "-" { -n } else { n }).to_string(),
                    )
                }
                Expr::Literal(Value::String(s)) => ("fixed".into(), s.clone()),
                Expr::Literal(v) if v.is_number() => ("fixed".into(), v.as_f64()?.to_string()),
                _ => return None,
            };
            Some(Condition {
                field,
                source,
                value,
            })
        };
        let mut draft = LookupDraft {
            collection: String::new(),
            op: op.clone(),
            field: String::new(),
            conditions: Vec::new(),
            fallback: String::new(),
            empty_when_missing,
            steps,
        };
        let mut collections = Vec::new();
        match op.as_str() {
            "XLOOKUP" => {
                let (c1, key) = column(&args[1])?;
                let (c2, field) = column(&args[2])?;
                collections.extend([c1, c2]);
                draft.field = field;
                draft.conditions.push(condition(key, &args[0])?);
                if let Some(fallback) = args.get(3) {
                    draft.fallback = match fallback {
                        Expr::Literal(Value::String(s)) => s.clone(),
                        Expr::Literal(v) if v.is_number() => v.as_f64()?.to_string(),
                        _ => return None,
                    };
                }
            }
            "SUMIFS" | "AVERAGEIFS" | "MINIFS" | "MAXIFS" | "COUNTIFS" => {
                let pairs = if op == "COUNTIFS" {
                    &args[..]
                } else {
                    let (c, field) = column(&args[0])?;
                    collections.push(c);
                    draft.field = field;
                    &args[1..]
                };
                for pair in pairs.chunks(2) {
                    let (c, field) = column(&pair[0])?;
                    collections.push(c);
                    draft.conditions.push(condition(field, &pair[1])?);
                }
            }
            _ => return None,
        }
        draft.collection = collections.first()?.clone();
        if collections.iter().any(|c| *c != draft.collection) {
            return None;
        }
        // This also verifies the guard counts exactly the same records as the lookup,
        // and that math order, fallback types and operator precedence are unchanged.
        (vulse_model::inspect_formula(&draft.formula().ok()?).ok()?
            == vulse_model::inspect_formula(formula).ok()?)
        .then_some(draft)
    }

    /// The calculation in words.
    pub fn sentence(&self, inputs: &[FieldDef], collection: Option<&CollectionView>) -> String {
        let label = |name: &str| {
            collection
                .and_then(|c| c.blueprint.fields.iter().find(|f| f.name == name))
                .map(|f| f.display_label().to_string())
                .unwrap_or_else(|| name.to_string())
        };
        let own = |name: &str| {
            inputs
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.display_label().to_string())
                .unwrap_or_else(|| name.to_string())
        };
        let start = match self.op.as_str() {
            "COUNTIFS" => format!("Count the records in {}", self.collection),
            "XLOOKUP" => format!(
                "Take {} from the first record in {}",
                label(&self.field),
                self.collection
            ),
            op => format!(
                "{} {} of the records in {}",
                OPERATIONS
                    .iter()
                    .find(|(o, _)| *o == op)
                    .map(|(_, l)| *l)
                    .unwrap_or(op),
                label(&self.field),
                self.collection
            )
            .replacen("Sum ", "The sum of ", 1)
            .replacen("Average ", "The average of ", 1)
            .replacen("Smallest value ", "The smallest ", 1)
            .replacen("Largest value ", "The largest ", 1),
        };
        let conditions: Vec<String> = self
            .conditions
            .iter()
            .map(|c| {
                let value = if let Some(name) = c.source.strip_prefix("field:") {
                    let adjust = c.value.trim().parse::<f64>().unwrap_or(0.0);
                    let mut text = format!("this record's {}", own(name));
                    if adjust > 0.0 {
                        text.push_str(&format!(" + {adjust}"));
                    } else if adjust < 0.0 {
                        text.push_str(&format!(" − {}", -adjust));
                    }
                    text
                } else if c.value.trim().is_empty() {
                    "…".into()
                } else {
                    format!("“{}”", c.value)
                };
                format!("{} is {value}", label(&c.field))
            })
            .collect();
        let mut text = format!("{start} where {}.", conditions.join(" and "));
        for step in &self.steps {
            let operation = RESULT_OPERATIONS
                .iter()
                .find(|(op, _)| *op == step.op)
                .map(|(_, label)| *label)
                .unwrap_or(&step.op);
            let value = step
                .source
                .strip_prefix("field:")
                .map(|name| format!("this record's {}", own(name)))
                .unwrap_or_else(|| step.value.clone());
            text.push_str(&match step.op.as_str() {
                "ROUND" => format!(" Then round to {value} decimal places."),
                "ROUNDUP" => format!(" Then round away from zero to {value} decimal places."),
                "ROUNDDOWN" => format!(" Then round towards zero to {value} decimal places."),
                _ => format!(" Then {} {value}.", operation.to_lowercase()),
            });
        }
        if self.empty_when_missing {
            text.push_str(" Returns empty text only when no records match.");
        } else if self.op == "XLOOKUP" {
            text.push_str(&if self.fallback.trim().is_empty() {
                " Shows an error when nothing matches.".to_string()
            } else {
                format!(
                    " Uses “{}” before any math steps when nothing matches.",
                    self.fallback
                )
            });
        } else if self.op != "AVERAGEIFS" {
            text.push_str(" Starts with 0 when nothing matches, then applies any math steps.");
        }
        text
    }
}

#[derive(Properties, PartialEq)]
pub(crate) struct LookupBuilderProps {
    pub draft: LookupDraft,
    /// Other collections the formula can read.
    pub collections: Vec<CollectionView>,
    /// This collection's other fields.
    pub inputs: Vec<FieldDef>,
    pub on_change: Callback<LookupDraft>,
}

fn select_value(e: Event) -> String {
    e.target_unchecked_into::<HtmlSelectElement>().value()
}
fn input_value(e: InputEvent) -> String {
    e.target_unchecked_into::<HtmlInputElement>().value()
}

#[function_component(LookupBuilder)]
pub(crate) fn lookup_builder(props: &LookupBuilderProps) -> Html {
    let draft = &props.draft;
    let collection = props
        .collections
        .iter()
        .find(|c| c.name == draft.collection);
    let fields = collection.map(readable).unwrap_or_default();
    let update = |apply: fn(&mut LookupDraft, String)| {
        let draft = draft.clone();
        let change = props.on_change.clone();
        move |value: String| {
            let mut next = draft.clone();
            apply(&mut next, value);
            change.emit(next);
        }
    };
    let on_collection = {
        let collections = props.collections.clone();
        let change = props.on_change.clone();
        Callback::from(move |e: Event| {
            let name = select_value(e);
            if let Some(c) = collections.iter().find(|c| c.name == name) {
                change.emit(LookupDraft::new(c));
            }
        })
    };
    let on_op = {
        let set = update(|d, v| {
            d.op = v;
            if d.op == "XLOOKUP" {
                d.conditions.truncate(1);
            }
        });
        Callback::from(move |e: Event| set(select_value(e)))
    };
    let on_field = {
        let set = update(|d, v| d.field = v);
        Callback::from(move |e: Event| set(select_value(e)))
    };
    let on_fallback = {
        let set = update(|d, v| d.fallback = v);
        Callback::from(move |e: InputEvent| set(input_value(e)))
    };
    let condition = |index: usize, apply: fn(&mut Condition, String)| {
        let draft = draft.clone();
        let change = props.on_change.clone();
        move |value: String| {
            let mut next = draft.clone();
            apply(&mut next.conditions[index], value);
            change.emit(next);
        }
    };
    let add = {
        let draft = draft.clone();
        let change = props.on_change.clone();
        let first = fields.first().map(|f| f.name.clone()).unwrap_or_default();
        Callback::from(move |_| {
            let mut next = draft.clone();
            next.conditions.push(Condition {
                field: first.clone(),
                ..Condition::default()
            });
            change.emit(next);
        })
    };
    let value_fields: Vec<&&FieldDef> = fields
        .iter()
        .filter(|f| draft.op == "XLOOKUP" || numeric(f))
        .collect();
    html! {<div class="lookup-builder">
        <div class="calculation-inputs">
            <div class="calculation-input-wrap"><label>{"Collection"}
                <select aria-label="Lookup collection" onchange={on_collection}>
                    {for props.collections.iter().map(|c| html!{<option value={c.name.clone()} selected={c.name == draft.collection}>{c.name.clone()}</option>})}
                </select></label></div>
            <div class="calculation-input-wrap"><label>{"Calculate"}
                <select aria-label="Lookup calculation" onchange={on_op}>
                    {for OPERATIONS.iter().map(|(op, label)| html!{<option value={*op} selected={*op == draft.op}>{*label}</option>})}
                </select></label></div>
            if draft.op != "COUNTIFS" {
                <div class="calculation-input-wrap"><label>{if draft.op == "XLOOKUP" {"Field to return"} else {"Of field"}}
                    <select aria-label="Lookup field" onchange={on_field}>
                        <option value="" selected={draft.field.is_empty()}>{"Choose a field…"}</option>
                        {for value_fields.iter().map(|f| html!{<option value={f.name.clone()} selected={f.name == draft.field}>{format!("{} [{}]", f.display_label(), f.name)}</option>})}
                    </select></label>
                    if value_fields.is_empty() {<p class="muted">{"This collection has no number fields."}</p>}
                </div>
            }
        </div>
        <h4 class="lookup-heading">{"Matching records"}</h4>
        <div class="lookup-conditions">
            {for draft.conditions.iter().enumerate().map(|(index, c)| {
                let on_field = { let set = condition(index, |c, v| c.field = v); Callback::from(move |e: Event| set(select_value(e))) };
                let on_source = { let set = condition(index, |c, v| { c.source = v; c.value.clear(); }); Callback::from(move |e: Event| set(select_value(e))) };
                let on_value = { let set = condition(index, |c, v| c.value = v); Callback::from(move |e: InputEvent| set(input_value(e))) };
                let remove = { let draft = draft.clone(); let change = props.on_change.clone(); Callback::from(move |_| { let mut next = draft.clone(); next.conditions.remove(index); change.emit(next); }) };
                let n = index + 1;
                html!{<div class="lookup-condition" key={index}>
                    <label>{format!("Their field {n}")}
                        <select aria-label={format!("Condition {n} field")} onchange={on_field}>
                            {for fields.iter().map(|f| html!{<option value={f.name.clone()} selected={f.name == c.field}>{format!("{} [{}]", f.display_label(), f.name)}</option>})}
                        </select></label>
                    <span class="lookup-equals">{"equals"}</span>
                    <label>{"Value"}
                        <select aria-label={format!("Condition {n} value source")} onchange={on_source}>
                            <option value="" selected={c.source.is_empty()}>{"Choose…"}</option>
                            <optgroup label="This record">
                                {for props.inputs.iter().map(|f| html!{<option value={format!("field:{}", f.name)} selected={c.source == format!("field:{}", f.name)}>{format!("{} [{}]", f.display_label(), f.name)}</option>})}
                            </optgroup>
                            <option value="fixed" selected={c.source == "fixed"}>{"Fixed value"}</option>
                        </select></label>
                    if c.source == "fixed" {
                        <label>{"Fixed value"}<input aria-label={format!("Condition {n} fixed value")} value={c.value.clone()} oninput={on_value}/></label>
                    } else if c.source.starts_with("field:") {
                        <label>{"Plus or minus"}<input aria-label={format!("Condition {n} adjustment")} type="number" step="any" placeholder="0" value={c.value.clone()} oninput={on_value}/></label>
                    }
                    if draft.conditions.len() > 1 {<button type="button" class="text-button" aria-label={format!("Remove condition {n}")} onclick={remove}>{"Remove"}</button>}
                </div>}
            })}
        </div>
        if draft.op != "XLOOKUP" {<button type="button" class="secondary" onclick={add}>{"+ Add condition"}</button>}
        <h4 class="lookup-heading">{"When no records match"}</h4>
        <label class="lookup-empty"><input type="checkbox" checked={draft.empty_when_missing} onchange={Callback::from({let draft=draft.clone();let change=props.on_change.clone();move |e:Event|{let mut next=draft.clone();next.empty_when_missing=e.target_unchecked_into::<HtmlInputElement>().checked();change.emit(next);}})}/>{"Return empty text when no records match"}</label>
        <p class="muted">{"When enabled, missing matches return empty text and skip the math steps below. Matching values, including 0, are calculated normally."}</p>
        if draft.op=="XLOOKUP" && !draft.empty_when_missing {<label class="lookup-fallback">{"If nothing matches (optional)"}<input aria-label="Lookup fallback" placeholder="Leave empty to show an error" value={draft.fallback.clone()} oninput={on_fallback}/></label>}
        <h4 class="lookup-heading">{"Calculate with the result"}</h4>
        <p class="muted">{"Add steps to apply in order. Each step uses the result of the previous step."}</p>
        <div class="lookup-result-steps">{for draft.steps.iter().enumerate().map(|(index, step)| {
            let n = index + 1;
            let on_op = {let draft=draft.clone();let change=props.on_change.clone();Callback::from(move |e:Event|{let mut next=draft.clone();next.steps[index].op=select_value(e);change.emit(next);})};
            let on_source = {let draft=draft.clone();let change=props.on_change.clone();Callback::from(move |e:Event|{let mut next=draft.clone();next.steps[index].source=select_value(e);change.emit(next);})};
            let on_value = {let draft=draft.clone();let change=props.on_change.clone();Callback::from(move |e:InputEvent|{let mut next=draft.clone();next.steps[index].value=input_value(e);change.emit(next);})};
            let remove = {let draft=draft.clone();let change=props.on_change.clone();Callback::from(move |_|{let mut next=draft.clone();next.steps.remove(index);change.emit(next);})};
            html!{<div class="lookup-result-step" role="group" aria-label={format!("Math step {n}")} key={index}>
                <label>{format!("Step {n}")}<select aria-label={format!("Math step {n} operation")} onchange={on_op}>
                    {for RESULT_OPERATIONS.iter().map(|(op,label)|html!{<option value={*op} selected={step.op==*op}>{*label}</option>})}
                </select></label>
                <label>{if step.op.starts_with("ROUND"){"Decimal places"}else{"Value"}}<select aria-label={format!("Math step {n} source")} onchange={on_source}>
                    <option value="fixed" selected={step.source=="fixed"}>{"Fixed number"}</option>
                    <optgroup label="This record">{for props.inputs.iter().filter(|f|numeric(f)).map(|f|html!{<option value={format!("field:{}",f.name)} selected={step.source==format!("field:{}",f.name)}>{format!("{} [{}]",f.display_label(),f.name)}</option>})}</optgroup>
                </select></label>
                if step.source=="fixed" {<label>{"Number"}<input type="number" step="any" aria-label={format!("Math step {n} number")} value={step.value.clone()} oninput={on_value}/></label>}
                <button type="button" class="text-button" aria-label={format!("Remove math step {n}")} onclick={remove}>{"Remove"}</button>
            </div>}
        })}</div>
        <button type="button" class="secondary" onclick={Callback::from({let draft=draft.clone();let change=props.on_change.clone();move |_|{let mut next=draft.clone();next.steps.push(ResultStep::default());change.emit(next);}})}>{"+ Add math step"}</button>
        <p class="lookup-sentence">{draft.sentence(&props.inputs, collection)}</p>
    </div>}
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUARDED_SUM: &str = "=IF(COUNTIFS([aarsmelding.org_nummer]; [orgnummer]; [aarsmelding.aar]; [aar]) = 0; \"\"; SUMIFS([aarsmelding.aarsverk_ex_spesped]; [aarsmelding.org_nummer]; [orgnummer]; [aarsmelding.aar]; [aar]))";

    fn evaluate(
        formula: &str,
        org: &str,
        factor: f64,
    ) -> (Value, std::collections::BTreeMap<String, String>) {
        use serde_json::json;
        use std::sync::Arc;
        use vulse_model::{Blueprint, LookupColumn, Lookups, calculate_document_with};
        let blueprint: Blueprint = serde_json::from_value(json!({"fields":[
            {"name":"orgnummer", "type":"string"},
            {"name":"aar", "type":"number"},
            {"name":"factor", "type":"number"},
            {"name":"result", "type":"calculated", "formula":formula}
        ]}))
        .unwrap();
        let mut lookups = Lookups::default();
        for (name, values) in [
            (
                "org_nummer",
                vec![
                    json!("positive"),
                    json!("positive"),
                    json!("zero"),
                    json!("*"),
                    json!("*"),
                    json!("other-year"),
                ],
            ),
            (
                "aar",
                vec![
                    json!(2025),
                    json!(2025),
                    json!(2025),
                    json!(2025),
                    json!(2025),
                    json!(2024),
                ],
            ),
            (
                "aarsverk_ex_spesped",
                vec![
                    json!(4.2),
                    json!(8.3),
                    json!(0),
                    json!(0),
                    json!(9),
                    json!(7),
                ],
            ),
        ] {
            lookups.insert(
                "aarsmelding",
                name,
                Arc::new(LookupColumn::new("aarsmelding", values)),
            );
        }
        calculate_document_with(
            &blueprint,
            &json!({"orgnummer":org,"aar":2025,"factor":factor}),
            &lookups,
        )
    }

    #[test]
    fn empty_when_missing_preserves_real_zero_and_all_matching_conditions() {
        let draft = LookupDraft::from_formula(GUARDED_SUM).unwrap();
        assert!(draft.empty_when_missing);
        assert_eq!(draft.conditions.len(), 2);
        assert_eq!(draft.op, "SUMIFS");
        let formula = draft.formula().unwrap();
        assert_eq!(
            vulse_model::inspect_formula(&formula),
            vulse_model::inspect_formula(GUARDED_SUM)
        );
        for (org, expected) in [
            ("positive", serde_json::json!(12.5)),
            ("zero", serde_json::json!(0.0)),
            ("missing", serde_json::json!("")),
            ("other-year", serde_json::json!("")),
        ] {
            let (body, errors) = evaluate(&formula, org, 2.0);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(body["result"], expected, "{org}");
        }
        // A guard for different records must stay in the advanced editor.
        let different_guard = GUARDED_SUM.replacen(
            "[aarsmelding.aar]; [aar]",
            "[aarsmelding.aar]; [aar] - 1",
            1,
        );
        assert!(LookupDraft::from_formula(&different_guard).is_none());
        let different_fallback = GUARDED_SUM.replacen("\"\"", "\"Not found\"", 1);
        assert!(LookupDraft::from_formula(&different_fallback).is_none());
    }

    #[test]
    fn result_steps_preserve_order_and_keep_the_no_match_guard_outside_math() {
        let mut draft = LookupDraft::from_formula(GUARDED_SUM).unwrap();
        draft.steps = vec![
            ResultStep {
                op: "*".into(),
                source: "field:factor".into(),
                value: String::new(),
            },
            ResultStep {
                op: "+".into(),
                source: "fixed".into(),
                value: "1".into(),
            },
            ResultStep {
                op: "/".into(),
                source: "fixed".into(),
                value: "3".into(),
            },
            ResultStep {
                op: "ROUND".into(),
                source: "fixed".into(),
                value: "2".into(),
            },
        ];
        let formula = draft.formula().unwrap();
        assert_eq!(LookupDraft::from_formula(&formula), Some(draft.clone()));
        for (org, expected) in [
            ("positive", serde_json::json!(8.67)),
            ("zero", serde_json::json!(0.33)),
            ("missing", serde_json::json!("")),
        ] {
            let (body, errors) = evaluate(&formula, org, 2.0);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(body["result"], expected, "{org}");
        }
        // No matches skip math errors; errors on a real match are still reported.
        draft.steps[2].value = "0".into();
        let formula = draft.formula().unwrap();
        assert!(evaluate(&formula, "missing", 2.0).1.is_empty());
        assert!(evaluate(&formula, "positive", 2.0).1.contains_key("result"));
        for op in ["SUMIFS", "AVERAGEIFS", "MINIFS", "MAXIFS", "COUNTIFS"] {
            draft.op = op.into();
            let formula = draft.formula().unwrap();
            let restored = LookupDraft::from_formula(&formula).unwrap();
            assert_eq!(restored.steps, draft.steps);
            assert_eq!(restored.formula().unwrap(), formula);
        }
    }

    #[test]
    fn first_match_math_uses_literal_lookup_keys_and_preserves_errors() {
        let base =
            "=XLOOKUP([orgnummer], [aarsmelding.org_nummer], [aarsmelding.aarsverk_ex_spesped])";
        let mut draft = LookupDraft::from_formula(base).unwrap();
        draft.empty_when_missing = true;
        draft.steps = vec![ResultStep::default()];
        let formula = draft.formula().unwrap();
        assert_eq!(LookupDraft::from_formula(&formula), Some(draft.clone()));
        for (org, expected) in [
            ("positive", serde_json::json!(8.4)),
            ("zero", serde_json::json!(0.0)),
            ("*", serde_json::json!(0.0)),
            ("?", serde_json::json!("")),
            ("missing", serde_json::json!("")),
        ] {
            let (body, errors) = evaluate(&formula, org, 2.0);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(body["result"], expected, "{org}");
        }
        draft.steps[0].op = "/".into();
        draft.steps[0].value = "0".into();
        let formula = draft.formula().unwrap();
        assert!(evaluate(&formula, "positive", 2.0).1.contains_key("result"));
        assert!(evaluate(&formula, "missing", 2.0).1.is_empty());
    }

    #[test]
    fn math_operations_round_trip_with_fields_and_negative_numbers() {
        let mut draft = LookupDraft::from_formula(GUARDED_SUM).unwrap();
        for (op, _) in RESULT_OPERATIONS {
            for source in ["fixed", "field:factor"] {
                draft.steps = vec![ResultStep {
                    op: (*op).into(),
                    source: source.into(),
                    value: if source == "fixed" {
                        "-2".into()
                    } else {
                        String::new()
                    },
                }];
                for empty in [true, false] {
                    draft.empty_when_missing = empty;
                    let formula = draft.formula().unwrap();
                    assert_eq!(
                        LookupDraft::from_formula(&formula),
                        Some(draft.clone()),
                        "{formula}"
                    );
                }
            }
        }
        draft.steps = vec![ResultStep {
            value: "".into(),
            ..ResultStep::default()
        }];
        assert!(draft.formula().unwrap_err().starts_with("Math step 1:"));
        draft.steps[0].value = "NaN".into();
        assert!(draft.formula().is_err());
        // Neither swapping operands nor moving the guard inside math is silently rewritten.
        assert!(LookupDraft::from_formula("=2 / SUMIFS([a.x], [a.y], [z])").is_none());
        assert!(
            LookupDraft::from_formula(&format!("=({}) * 2", GUARDED_SUM.trim_start_matches('=')))
                .is_none()
        );
    }

    #[test]
    fn writes_and_reads_back_lookup_formulas() {
        let draft = LookupDraft {
            collection: "aarsmelding".into(),
            op: "SUMIFS".into(),
            field: "aarsverk".into(),
            conditions: vec![
                Condition {
                    field: "org_nummer".into(),
                    source: "field:orgnummer".into(),
                    value: String::new(),
                },
                Condition {
                    field: "aar".into(),
                    source: "field:aar".into(),
                    value: "-2".into(),
                },
                Condition {
                    field: "eiertype".into(),
                    source: "fixed".into(),
                    value: "Privat".into(),
                },
            ],
            fallback: String::new(),
            empty_when_missing: false,
            steps: Vec::new(),
        };
        let formula = draft.formula().unwrap();
        assert_eq!(
            formula,
            "=SUMIFS([aarsmelding.aarsverk], [aarsmelding.org_nummer], [orgnummer], [aarsmelding.aar], [aar] - 2, [aarsmelding.eiertype], \"Privat\")"
        );
        assert_eq!(LookupDraft::from_formula(&formula), Some(draft.clone()));
        // Semicolons and spacing from typed formulas read back too.
        assert!(LookupDraft::from_formula("=SUMIFS([a.x];[a.y];[z]+1)").is_some());
        let mut lookup = draft.clone();
        lookup.op = "XLOOKUP".into();
        lookup.conditions.truncate(1);
        lookup.fallback = "Not found".into();
        let formula = lookup.formula().unwrap();
        assert_eq!(
            formula,
            "=XLOOKUP([orgnummer], [aarsmelding.org_nummer], [aarsmelding.aarsverk], \"Not found\")"
        );
        assert_eq!(LookupDraft::from_formula(&formula), Some(lookup));
        let mut count = draft;
        count.op = "COUNTIFS".into();
        count.field.clear();
        assert_eq!(
            LookupDraft::from_formula(&count.formula().unwrap()),
            Some(count)
        );
        for other in [
            "=SUMIFS([a.x], [b.y], 1)",
            "=SUM([a.x])",
            "=SUMIFS([a.x], [a.y], [z] * 2)",
            "=[a]",
        ] {
            assert_eq!(LookupDraft::from_formula(other), None, "{other}");
        }
    }

    #[test]
    fn incomplete_drafts_explain_what_is_missing() {
        let mut draft = LookupDraft {
            collection: "a".into(),
            op: "SUMIFS".into(),
            field: "x".into(),
            conditions: vec![Condition {
                field: "y".into(),
                ..Condition::default()
            }],
            fallback: String::new(),
            empty_when_missing: false,
            steps: Vec::new(),
        };
        assert!(
            draft
                .formula()
                .unwrap_err()
                .contains("choose a field of this record")
        );
        draft.conditions[0].source = "field:z".into();
        draft.conditions[0].value = "two".into();
        assert!(draft.formula().unwrap_err().contains("must be a number"));
    }
}
