//! Guided calculations that read another collection: a sum, average,
//! minimum, maximum or count of matching records, or a value from the first
//! matching record. Builds `SUMIFS`-style formulas and reads them back.

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
        let Expr::Call(op, args) = vulse_model::inspect_formula(formula).ok()? else {
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
        // Only when writing it back gives the same formula.
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
        if self.op == "XLOOKUP" {
            text.push_str(&if self.fallback.trim().is_empty() {
                " Shows an error when nothing matches.".to_string()
            } else {
                format!(" Shows “{}” when nothing matches.", self.fallback)
            });
        } else if self.op != "COUNTIFS" && self.op != "AVERAGEIFS" {
            text.push_str(" Gives 0 when nothing matches.");
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
        else {<label class="lookup-fallback">{"If nothing matches (optional)"}<input aria-label="Lookup fallback" placeholder="Leave empty to show an error" value={draft.fallback.clone()} oninput={on_fallback}/></label>}
        <p class="lookup-sentence">{draft.sentence(&props.inputs, collection)}</p>
    </div>}
}

#[cfg(test)]
mod tests {
    use super::*;

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
