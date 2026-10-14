//! Summary mode of the records view: group records, aggregate values, derive
//! values with formulas, and show the result as a table and a chart.

use serde_json::Value;
use vulse_model::{
    ChartKind, CollectionView, DateBucket, FieldDef, FieldType, MAX_CHART_SERIES, MAX_GROUP_FIELDS,
    MAX_MEASURES, RecordFilter, RecordQuery, SummaryChart, SummaryGroup, SummaryMeasure, SummaryOp,
    SummaryRequest, SummaryResult, SummaryRow, SummarySpec,
};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

use crate::api;
use crate::formula_input::{FormulaInput, Suggestion};

/// A useful first summary: grouped by the first text, reference or date
/// field, summing the first number field.
pub fn default_spec(fields: &[FieldDef]) -> SummarySpec {
    let group = fields.iter().find(|f| {
        matches!(
            f.kind,
            FieldType::String | FieldType::Reference | FieldType::Date
        )
    });
    let number = fields.iter().find(|f| f.kind == FieldType::Number);
    // The table always shows a Records column, so start with a sum when possible.
    let measures = vec![match number {
        Some(number) => SummaryMeasure {
            name: format!("total_{}", number.name),
            op: SummaryOp::Sum,
            field: Some(number.name.clone()),
            formula: None,
        },
        None => SummaryMeasure {
            name: "records".into(),
            op: SummaryOp::Count,
            field: None,
            formula: None,
        },
    }];
    let chart_measure = measures.last().map(|m| m.name.clone()).unwrap_or_default();
    SummarySpec {
        group_by: group
            .map(|f| SummaryGroup {
                field: f.name.clone(),
                bucket: (f.kind == FieldType::Date).then_some(DateBucket::Month),
            })
            .into_iter()
            .collect(),
        measures,
        sort: None,
        descending: false,
        chart: Some(SummaryChart {
            kind: if group.is_some_and(|f| f.kind == FieldType::Date) {
                ChartKind::Line
            } else {
                ChartKind::Bar
            },
            measures: vec![chart_measure],
        }),
    }
}

#[derive(Properties, PartialEq)]
pub struct SummaryPanelProps {
    pub collection: CollectionView,
    pub query: RecordQuery,
    pub spec: SummarySpec,
    pub on_change: Callback<SummarySpec>,
    /// Shows the records of one group in the table.
    pub on_drill: Callback<Vec<RecordFilter>>,
}

#[function_component(SummaryPanel)]
pub fn summary_panel(props: &SummaryPanelProps) -> Html {
    let result = use_state(|| None::<SummaryResult>);
    let error = use_state(String::new);
    let loading = use_state(|| false);
    let epoch = use_mut_ref(|| 0usize);
    let fields = &props.collection.blueprint.fields;
    let invalid = vulse_model::validate_summary(&props.collection.blueprint, &props.spec).err();
    {
        let (result, error, loading, epoch) = (
            result.clone(),
            error.clone(),
            loading.clone(),
            epoch.clone(),
        );
        let id = props.collection.id.clone();
        let request = SummaryRequest {
            query: props.query.clone(),
            summary: props.spec.clone(),
        };
        let valid = invalid.is_none();
        use_effect_with((id, request, valid), move |(id, request, valid)| {
            *epoch.borrow_mut() += 1;
            let generation = *epoch.borrow();
            let (id, request) = (id.clone(), request.clone());
            let timer = (*valid).then(|| {
                loading.set(true);
                gloo_timers::callback::Timeout::new(250, move || {
                    wasm_bindgen_futures::spawn_local(async move {
                        let outcome = api::summarize(&id, &request).await;
                        if *epoch.borrow() != generation {
                            return;
                        }
                        match outcome {
                            Ok(next) => {
                                result.set(Some(next));
                                error.set(String::new());
                            }
                            Err(e) => error.set(e),
                        }
                        loading.set(false);
                    });
                })
            });
            move || drop(timer)
        });
    }
    let spec = props.spec.clone();
    let change = props.on_change.clone();
    html! {<div class="summary-panel">
        <SummaryBuilder fields={fields.clone()} spec={spec.clone()} on_change={change.clone()}/>
        if let Some(message) = &invalid {
            <p class="error" role="alert">{message.clone()}</p>
        } else if !error.is_empty() {
            <p class="error" role="alert">{(*error).clone()}</p>
        }
        if let Some(result) = (*result).clone().filter(|_| invalid.is_none()) {
            <div class={classes!("summary-results", loading.then_some("refreshing"))}>
                if let Some(chart) = &spec.chart {
                    <SummaryChartView spec={spec.clone()} chart={chart.clone()} result={result.clone()} fields={fields.clone()}/>
                }
                {summary_table(&props.collection, &spec, &result, &props.on_drill)}
            </div>
        } else if invalid.is_none() {
            <p class="muted summary-loading" role="status">{"Calculating…"}</p>
        }
    </div>}
}

fn field_label(fields: &[FieldDef], name: &str) -> String {
    fields
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.display_label().to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Plain numbers with up to two decimals; text as is.
fn format_value(value: &Value) -> String {
    match value {
        Value::Null => "—".into(),
        Value::Number(n) => n
            .as_f64()
            .map(|f| {
                if f.fract() == 0.0 {
                    format!("{f:.0}")
                } else {
                    let text = format!("{f:.2}");
                    text.trim_end_matches('0').trim_end_matches('.').to_string()
                }
            })
            .unwrap_or_else(|| n.to_string()),
        other => vulse_model::display_value(other),
    }
}

fn summary_table(
    collection: &CollectionView,
    spec: &SummarySpec,
    result: &SummaryResult,
    on_drill: &Callback<Vec<RecordFilter>>,
) -> Html {
    let fields = &collection.blueprint.fields;
    let export = {
        let (spec, result, name) = (spec.clone(), result.clone(), collection.name.clone());
        let fields = fields.clone();
        Callback::from(move |_| {
            let csv = summary_csv(&fields, &spec, &result);
            let _ = crate::records::download(
                &format!("{name}-summary.csv"),
                csv.as_bytes(),
                "text/csv",
            );
        })
    };
    let heading = |group: &SummaryGroup| {
        let label = field_label(fields, &group.field);
        match group.bucket {
            Some(DateBucket::Day) => format!("{label} (day)"),
            Some(DateBucket::Month) => format!("{label} (month)"),
            Some(DateBucket::Year) => format!("{label} (year)"),
            None => label,
        }
    };
    html! {<div class="summary-table">
        <div class="summary-table-heading">
            <span class="muted" role="status">{
                if result.truncated {
                    format!("Showing {} of {} groups", result.rows.len(), result.group_count)
                } else if spec.group_by.is_empty() {
                    format!("{} records", result.totals.count)
                } else {
                    format!("{} groups · {} records", result.group_count, result.totals.count)
                }
            }</span>
            <button type="button" class="secondary" onclick={export}>{"Export summary CSV"}</button>
        </div>
        <div class="table-scroll"><table>
            <thead><tr>
                {for spec.group_by.iter().map(|g| html!{<th>{heading(g)}</th>})}
                <th class="number">{"Records"}</th>
                {for spec.measures.iter().map(|m| html!{<th class="number" title={describe(fields, m)}>{m.name.clone()}</th>})}
                if !spec.group_by.is_empty() {<th><span class="visually-hidden">{"Actions"}</span></th>}
            </tr></thead>
            <tbody>
                {for result.rows.iter().map(|row| {
                    let drill = {
                        let filters = drill_filters(fields, spec, row);
                        let on_drill = on_drill.clone();
                        Callback::from(move |_| on_drill.emit(filters.clone()))
                    };
                    html!{<tr>
                        {for row.labels.iter().map(|label| html!{<td>{label.clone()}</td>})}
                        <td class="number">{row.count}</td>
                        {for spec.measures.iter().map(|m| match row.errors.get(&m.name) {
                            Some(error) => html!{<td class="number cell-error" title={error.clone()}>{"Error"}</td>},
                            None => html!{<td class="number">{format_value(row.values.get(&m.name).unwrap_or(&Value::Null))}</td>},
                        })}
                        if !spec.group_by.is_empty() {
                            <td><button type="button" class="text-button" aria-label={format!("View records for {}", row.labels.join(" · "))} onclick={drill}>{"View records →"}</button></td>
                        }
                    </tr>}
                })}
            </tbody>
            if !spec.group_by.is_empty() {
                <tfoot><tr>
                    <th colspan={spec.group_by.len().to_string()}>{"All records"}</th>
                    <th class="number">{result.totals.count}</th>
                    {for spec.measures.iter().map(|m| match result.totals.errors.get(&m.name) {
                        Some(error) => html!{<th class="number cell-error" title={error.clone()}>{"Error"}</th>},
                        None => html!{<th class="number">{format_value(result.totals.values.get(&m.name).unwrap_or(&Value::Null))}</th>},
                    })}
                    <th></th>
                </tr></tfoot>
            }
        </table></div>
    </div>}
}

fn describe(fields: &[FieldDef], measure: &SummaryMeasure) -> String {
    match (measure.op, &measure.field, &measure.formula) {
        (SummaryOp::Formula, _, Some(formula)) => formula.clone(),
        (SummaryOp::Count, None, _) => "Records in the group".into(),
        (op, Some(field), _) => format!("{} of {}", op.label(), field_label(fields, field)),
        (op, _, _) => op.label().into(),
    }
}

fn summary_csv(fields: &[FieldDef], spec: &SummarySpec, result: &SummaryResult) -> String {
    let mut headers: Vec<String> = spec
        .group_by
        .iter()
        .map(|g| field_label(fields, &g.field))
        .collect();
    headers.push("records".into());
    headers.extend(spec.measures.iter().map(|m| m.name.clone()));
    let mut lines = vec![csv_line(&headers)];
    for row in result.rows.iter() {
        let mut cells = row.labels.clone();
        cells.push(row.count.to_string());
        cells.extend(spec.measures.iter().map(|m| match row.values.get(&m.name) {
            Some(Value::Null) | None => String::new(),
            Some(Value::String(text)) => text.clone(),
            Some(value) => value.to_string(),
        }));
        lines.push(csv_line(&cells));
    }
    lines.join("\r\n") + "\r\n"
}

/// Quotes cells as needed and defuses spreadsheet formulas, like the record export.
fn csv_line(cells: &[String]) -> String {
    cells
        .iter()
        .map(|cell| {
            let cell = if cell.trim_start().starts_with(['=', '+', '-', '@'])
                && cell.parse::<f64>().is_err()
            {
                format!("'{cell}")
            } else {
                cell.clone()
            };
            if cell.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", cell.replace('"', "\"\""))
            } else {
                cell
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Filters that select one group's records in the table.
fn drill_filters(fields: &[FieldDef], spec: &SummarySpec, row: &SummaryRow) -> Vec<RecordFilter> {
    let mut filters = Vec::new();
    for (group, value) in spec.group_by.iter().zip(&row.group) {
        let filter = |op: &str, value: Value| RecordFilter {
            field: group.field.clone(),
            op: op.into(),
            value,
            or: false,
        };
        match (group.bucket, value) {
            (_, Value::Null) => filters.push(filter("empty", Value::String(String::new()))),
            (Some(bucket), Value::String(key)) => {
                let (start, end) = bucket_range(bucket, key);
                filters.push(filter("gte", Value::String(start)));
                filters.push(filter("lte", Value::String(end)));
            }
            _ => {
                // Calculated values are matched by their text, like other filters.
                let calculated = fields
                    .iter()
                    .any(|f| f.name == group.field && f.kind == FieldType::Calculated);
                let value = if calculated && value.is_number() {
                    Value::String(vulse_model::display_value(value))
                } else {
                    value.clone()
                };
                filters.push(filter("eq", value));
            }
        }
    }
    filters
}

/// First and last day of a day, month or year bucket key.
fn bucket_range(bucket: DateBucket, key: &str) -> (String, String) {
    match bucket {
        DateBucket::Day => (key.into(), key.into()),
        DateBucket::Year => (format!("{key}-01-01"), format!("{key}-12-31")),
        DateBucket::Month => {
            let year: i32 = key.get(..4).and_then(|y| y.parse().ok()).unwrap_or(1970);
            let month: u32 = key.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            let last = match month {
                2 if leap => 29,
                2 => 28,
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            };
            (format!("{key}-01"), format!("{key}-{last:02}"))
        }
    }
}

#[derive(Properties, PartialEq)]
struct SummaryBuilderProps {
    fields: Vec<FieldDef>,
    spec: SummarySpec,
    on_change: Callback<SummarySpec>,
}

#[function_component(SummaryBuilder)]
fn summary_builder(props: &SummaryBuilderProps) -> Html {
    let fields = &props.fields;
    let spec = &props.spec;
    let update = |apply: Box<dyn Fn(&mut SummarySpec)>| {
        let spec = spec.clone();
        let on_change = props.on_change.clone();
        move || {
            let mut next = spec.clone();
            apply(&mut next);
            on_change.emit(next);
        }
    };
    let groupable: Vec<&FieldDef> = fields
        .iter()
        .filter(|f| {
            matches!(
                f.kind,
                FieldType::String
                    | FieldType::Number
                    | FieldType::Boolean
                    | FieldType::Date
                    | FieldType::Reference
                    | FieldType::Calculated
            )
        })
        .collect();
    let select_value = |e: Event| e.target_unchecked_into::<HtmlSelectElement>().value();
    let input_value = |e: InputEvent| e.target_unchecked_into::<HtmlInputElement>().value();

    let groups = spec.group_by.iter().enumerate().map(|(index, group)| {
        let is_date = fields.iter().any(|f| f.name == group.field && f.kind == FieldType::Date);
        let on_field = {
            let (spec, on_change, fields) = (spec.clone(), props.on_change.clone(), fields.clone());
            Callback::from(move |e: Event| {
                let field = select_value(e);
                let date = fields.iter().any(|f| f.name == field && f.kind == FieldType::Date);
                let mut next = spec.clone();
                next.group_by[index] = SummaryGroup {
                    field,
                    bucket: date.then_some(DateBucket::Month),
                };
                on_change.emit(next);
            })
        };
        let on_bucket = {
            let (spec, on_change) = (spec.clone(), props.on_change.clone());
            Callback::from(move |e: Event| {
                let mut next = spec.clone();
                next.group_by[index].bucket = match select_value(e).as_str() {
                    "day" => Some(DateBucket::Day),
                    "year" => Some(DateBucket::Year),
                    "month" => Some(DateBucket::Month),
                    _ => None,
                };
                on_change.emit(next);
            })
        };
        let remove = {
            let run = update(Box::new(move |next: &mut SummarySpec| {
                let removed = next.group_by.remove(index);
                if next.sort.as_deref() == Some(removed.field.as_str()) {
                    next.sort = None;
                }
            }));
            Callback::from(move |_| run())
        };
        html! {<div class="summary-row" key={format!("group-{index}")}>
            <label>{format!("Group {}", index + 1)}
                <select aria-label={format!("Group {} field", index + 1)} onchange={on_field}>
                    {for groupable.iter().map(|f| html!{<option value={f.name.clone()} selected={f.name == group.field}>{f.display_label().to_string()}</option>})}
                </select>
            </label>
            if is_date {
                <label>{"By"}
                    <select aria-label={format!("Group {} date grouping", index + 1)} onchange={on_bucket}>
                        {for [("day","Day"),("month","Month"),("year","Year"),("","Exact date")].into_iter().map(|(value,label)| {
                            let current = match group.bucket { Some(DateBucket::Day)=>"day", Some(DateBucket::Month)=>"month", Some(DateBucket::Year)=>"year", None=>"" };
                            html!{<option value={value} selected={current==value}>{label}</option>}
                        })}
                    </select>
                </label>
            }
            <button type="button" class="secondary" aria-label={format!("Remove group {}", index + 1)} onclick={remove}>{"Remove"}</button>
        </div>}
    });

    let measures = spec.measures.iter().enumerate().map(|(index, measure)| {
        let candidates: Vec<&FieldDef> = fields
            .iter()
            .filter(|f| {
                let numeric = matches!(f.kind, FieldType::Number | FieldType::Calculated);
                match measure.op {
                    SummaryOp::Sum | SummaryOp::Average => numeric,
                    SummaryOp::Min | SummaryOp::Max => numeric || f.kind == FieldType::Date,
                    _ => !matches!(f.kind, FieldType::PortableText | FieldType::Json),
                }
            })
            .collect();
        let on_name = {
            let (spec, on_change) = (spec.clone(), props.on_change.clone());
            Callback::from(move |e: InputEvent| {
                let mut next = spec.clone();
                let old = next.measures[index].name.clone();
                let name = input_value(e).trim().to_string();
                next.measures[index].name = name.clone();
                // Keep the chart and sort pointing at the renamed value.
                if next.sort.as_deref() == Some(old.as_str()) {
                    next.sort = Some(name.clone());
                }
                if let Some(chart) = &mut next.chart {
                    for series in &mut chart.measures {
                        if *series == old {
                            *series = name.clone();
                        }
                    }
                }
                on_change.emit(next);
            })
        };
        let on_op = {
            let (spec, on_change, fields) = (spec.clone(), props.on_change.clone(), fields.clone());
            Callback::from(move |e: Event| {
                let mut next = spec.clone();
                let op = SummaryOp::parse(&select_value(e)).unwrap_or(SummaryOp::Count);
                let measure = &mut next.measures[index];
                measure.op = op;
                let fits = |f: &FieldDef| {
                    let numeric = matches!(f.kind, FieldType::Number | FieldType::Calculated);
                    match op {
                        SummaryOp::Sum | SummaryOp::Average => numeric,
                        SummaryOp::Min | SummaryOp::Max => numeric || f.kind == FieldType::Date,
                        _ => !matches!(f.kind, FieldType::PortableText | FieldType::Json),
                    }
                };
                match op {
                    SummaryOp::Formula => {
                        measure.field = None;
                        if measure.formula.is_none() {
                            measure.formula = Some("=".into());
                        }
                    }
                    SummaryOp::Count => measure.formula = None,
                    _ => {
                        measure.formula = None;
                        let current = measure
                            .field
                            .as_ref()
                            .and_then(|name| fields.iter().find(|f| f.name == *name))
                            .filter(|f| fits(f));
                        measure.field = current
                            .or_else(|| fields.iter().find(|f| fits(f)))
                            .map(|f| f.name.clone());
                    }
                }
                on_change.emit(next);
            })
        };
        let on_field = {
            let (spec, on_change) = (spec.clone(), props.on_change.clone());
            Callback::from(move |e: Event| {
                let mut next = spec.clone();
                let value = select_value(e);
                next.measures[index].field = (!value.is_empty()).then_some(value);
                on_change.emit(next);
            })
        };
        let on_formula = {
            let (spec, on_change) = (spec.clone(), props.on_change.clone());
            Callback::from(move |formula: String| {
                let mut next = spec.clone();
                next.measures[index].formula = Some(formula);
                on_change.emit(next);
            })
        };
        let remove = {
            let run = update(Box::new(move |next: &mut SummarySpec| {
                let removed = next.measures.remove(index);
                if next.sort.as_deref() == Some(removed.name.as_str()) {
                    next.sort = None;
                }
                if let Some(chart) = &mut next.chart {
                    chart.measures.retain(|m| *m != removed.name);
                    if chart.measures.is_empty() {
                        next.chart = None;
                    }
                }
            }));
            Callback::from(move |_| run())
        };
        // Other values can be used in the formula by name.
        let others: Vec<Suggestion> = spec
            .measures
            .iter()
            .filter(|m| m.name != measure.name && !m.name.is_empty())
            .map(|m| Suggestion {
                name: m.name.clone(),
                label: m.name.clone(),
                detail: describe(&props.fields, m),
            })
            .collect();
        html! {<div class="summary-row measure-row" key={format!("measure-{index}")}>
            <label>{"Name"}
                <input aria-label={format!("Value {} name", index + 1)} value={measure.name.clone()} oninput={on_name}/>
            </label>
            <label>{"Calculate"}
                <select aria-label={format!("Value {} calculation", index + 1)} onchange={on_op}>
                    {for SummaryOp::ALL.into_iter().map(|op| html!{<option value={op.as_str()} selected={op == measure.op}>{op.label()}</option>})}
                </select>
            </label>
            if measure.op == SummaryOp::Formula {
                <FormulaInput label={format!("Value {} formula", index + 1)} rows={2} library={true} placeholder="=[total_revenue]/[records]" value={measure.formula.clone().unwrap_or_default()} suggestions={others.clone()} on_change={on_formula}/>
            } else {
                <label>{"Of field"}
                    <select aria-label={format!("Value {} field", index + 1)} onchange={on_field}>
                        if measure.op == SummaryOp::Count {
                            <option value="" selected={measure.field.is_none()}>{"Records"}</option>
                        }
                        {for candidates.iter().map(|f| html!{<option value={f.name.clone()} selected={measure.field.as_deref() == Some(f.name.as_str())}>{f.display_label().to_string()}</option>})}
                    </select>
                </label>
            }
            <button type="button" class="secondary" aria-label={format!("Remove value {}", index + 1)} disabled={spec.measures.len() == 1} onclick={remove}>{"Remove"}</button>
        </div>}
    });

    let add_group = {
        let first = groupable
            .iter()
            .find(|f| !spec.group_by.iter().any(|g| g.field == f.name))
            .map(|f| (f.name.clone(), f.kind == FieldType::Date));
        let run = update(Box::new(move |next: &mut SummarySpec| {
            if let Some((field, date)) = first.clone() {
                next.group_by.push(SummaryGroup {
                    field,
                    bucket: date.then_some(DateBucket::Month),
                });
            }
        }));
        Callback::from(move |_| run())
    };
    let add_value = |op: SummaryOp| {
        let mut n = spec.measures.len() + 1;
        while spec.measures.iter().any(|m| m.name == format!("value_{n}")) {
            n += 1;
        }
        let name = format!("value_{n}");
        let number = fields
            .iter()
            .find(|f| f.kind == FieldType::Number)
            .map(|f| f.name.clone());
        let run = update(Box::new(move |next: &mut SummarySpec| {
            next.measures.push(SummaryMeasure {
                name: name.clone(),
                op,
                field: if op == SummaryOp::Sum {
                    number.clone()
                } else {
                    None
                },
                formula: (op == SummaryOp::Formula).then(|| "=".to_string()),
            });
        }));
        Callback::from(move |_| run())
    };
    let sort_options: Vec<(String, String)> = spec
        .group_by
        .iter()
        .map(|g| (g.field.clone(), field_label(fields, &g.field)))
        .chain(
            spec.measures
                .iter()
                .map(|m| (m.name.clone(), m.name.clone())),
        )
        .collect();
    let on_sort = {
        let (spec, on_change) = (spec.clone(), props.on_change.clone());
        Callback::from(move |e: Event| {
            let mut next = spec.clone();
            let value = select_value(e);
            next.sort = (!value.is_empty()).then_some(value);
            on_change.emit(next);
        })
    };
    let on_order = {
        let (spec, on_change) = (spec.clone(), props.on_change.clone());
        Callback::from(move |e: Event| {
            let mut next = spec.clone();
            next.descending = select_value(e) == "desc";
            on_change.emit(next);
        })
    };
    let on_chart = {
        let (spec, on_change) = (spec.clone(), props.on_change.clone());
        Callback::from(move |e: Event| {
            let mut next = spec.clone();
            let kind = match select_value(e).as_str() {
                "bar" => Some(ChartKind::Bar),
                "line" => Some(ChartKind::Line),
                _ => None,
            };
            next.chart = kind.map(|kind| SummaryChart {
                kind,
                measures: spec
                    .chart
                    .as_ref()
                    .map(|c| c.measures.clone())
                    .filter(|m| !m.is_empty())
                    .unwrap_or_else(|| {
                        spec.measures
                            .iter()
                            .take(1)
                            .map(|m| m.name.clone())
                            .collect()
                    }),
            });
            on_change.emit(next);
        })
    };
    let chart_kind = spec.chart.as_ref().map(|c| c.kind);
    let charted: Vec<String> = spec
        .chart
        .as_ref()
        .map(|c| c.measures.clone())
        .unwrap_or_default();
    html! {<details class="summary-builder" open=true>
        <summary>{"Summarize"}<span class="muted">{
            if spec.group_by.is_empty() { " · all records".to_string() }
            else { format!(" · by {}", spec.group_by.iter().map(|g| field_label(fields, &g.field)).collect::<Vec<_>>().join(", ")) }
        }</span></summary>
        <div class="summary-section">
            <h3>{"Group records by"}</h3>
            if spec.group_by.is_empty() {<p class="muted">{"No grouping: values are calculated over all matching records."}</p>}
            {for groups}
            <button type="button" class="secondary" disabled={spec.group_by.len() >= MAX_GROUP_FIELDS || groupable.len() <= spec.group_by.len()} onclick={add_group}>{"+ Add group"}</button>
        </div>
        <div class="summary-section">
            <h3>{"Values"}</h3>
            {for measures}
            <div class="row">
                <button type="button" class="secondary" disabled={spec.measures.len() >= MAX_MEASURES} onclick={add_value(SummaryOp::Sum)}>{"+ Add value"}</button>
                <button type="button" class="secondary" disabled={spec.measures.len() >= MAX_MEASURES} onclick={add_value(SummaryOp::Formula)}>{"+ Add formula"}</button>
            </div>
        </div>
        <div class="summary-section summary-options">
            <label>{"Sort groups by"}
                <select aria-label="Sort groups by" onchange={on_sort}>
                    <option value="" selected={spec.sort.is_none()}>{"Group values"}</option>
                    {for sort_options.iter().map(|(value, label)| html!{<option value={value.clone()} selected={spec.sort.as_deref() == Some(value.as_str())}>{label.clone()}</option>})}
                </select>
            </label>
            <label>{"Order"}
                <select aria-label="Summary order" onchange={on_order}>
                    <option value="asc" selected={!spec.descending}>{"Ascending"}</option>
                    <option value="desc" selected={spec.descending}>{"Descending"}</option>
                </select>
            </label>
            <label>{"Chart"}
                <select aria-label="Chart type" onchange={on_chart}>
                    <option value="" selected={chart_kind.is_none()}>{"No chart"}</option>
                    <option value="bar" selected={chart_kind == Some(ChartKind::Bar)}>{"Bar chart"}</option>
                    <option value="line" selected={chart_kind == Some(ChartKind::Line)}>{"Line chart"}</option>
                </select>
            </label>
            if chart_kind.is_some() {
                <fieldset class="chart-series">
                    <legend>{format!("Chart values (up to {MAX_CHART_SERIES})")}</legend>
                    {for spec.measures.iter().map(|m| {
                        let checked = charted.contains(&m.name);
                        let toggle = {
                            let (spec, on_change, name) = (spec.clone(), props.on_change.clone(), m.name.clone());
                            Callback::from(move |e: Event| {
                                let mut next = spec.clone();
                                if let Some(chart) = &mut next.chart {
                                    chart.measures.retain(|n| *n != name);
                                    if e.target_unchecked_into::<HtmlInputElement>().checked() {
                                        chart.measures.push(name.clone());
                                        // Series follow the order of the values.
                                        let order: Vec<String> = next.measures.iter().map(|m| m.name.clone()).collect();
                                        chart.measures.sort_by_key(|n| order.iter().position(|o| o == n));
                                    }
                                }
                                on_change.emit(next);
                            })
                        };
                        html!{<label class="check"><input type="checkbox" checked={checked} disabled={!checked && charted.len() >= MAX_CHART_SERIES} onchange={toggle}/>{m.name.clone()}</label>}
                    })}
                </fieldset>
            }
        </div>
    </details>}
}

// ---- Chart -----------------------------------------------------------------

const CHART_HEIGHT: f64 = 260.0;
const PLOT_TOP: f64 = 16.0;
const PLOT_BOTTOM: f64 = 40.0;
const PLOT_LEFT: f64 = 56.0;
const PLOT_RIGHT: f64 = 16.0;

#[derive(Properties, PartialEq)]
struct ChartProps {
    spec: SummarySpec,
    chart: SummaryChart,
    result: SummaryResult,
    fields: Vec<FieldDef>,
}

/// Bar or line chart of up to four values per group, on one shared axis.
#[function_component(SummaryChartView)]
fn summary_chart(props: &ChartProps) -> Html {
    let hovered = use_state(|| None::<usize>);
    // The chart fills its card; groups that don't fit scroll sideways.
    let scroller = use_node_ref();
    let available = use_state(|| 0.0_f64);
    {
        let (scroller, available) = (scroller.clone(), available.clone());
        use_effect_with((), move |_| {
            use wasm_bindgen::JsCast;
            let observer = scroller.cast::<web_sys::Element>().and_then(|element| {
                available.set(element.client_width() as f64);
                let callback = wasm_bindgen::closure::Closure::<dyn FnMut(js_sys::Array)>::new(
                    move |entries: js_sys::Array| {
                        if let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>()
                        {
                            available.set(entry.target().client_width() as f64);
                        }
                    },
                );
                let observer =
                    web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
                observer.observe(&element);
                Some((observer, callback))
            });
            move || {
                if let Some((observer, _callback)) = observer {
                    observer.disconnect();
                }
            }
        });
    }
    let rows = &props.result.rows;
    let series: Vec<&String> = props
        .chart
        .measures
        .iter()
        .filter(|name| props.spec.measures.iter().any(|m| m.name == **name))
        .collect();
    if rows.is_empty() || series.is_empty() {
        return html! {<p class="muted">{"Nothing to chart yet."}</p>};
    }
    let value = |row: &SummaryRow, name: &str| row.values.get(name).and_then(Value::as_f64);
    let all: Vec<f64> = rows
        .iter()
        .flat_map(|row| series.iter().filter_map(move |name| value(row, name)))
        .collect();
    if all.is_empty() {
        return html! {<p class="muted">{"The charted values have no numbers to plot."}</p>};
    }
    let low = all.iter().cloned().fold(0.0_f64, f64::min);
    let high = all.iter().cloned().fold(0.0_f64, f64::max);
    let ticks = nice_ticks(low, high);
    let (min, max) = (ticks[0], *ticks.last().unwrap());
    let line = props.chart.kind == ChartKind::Line;
    let room = if *available > 0.0 { *available } else { 640.0 };
    let fit = (room - PLOT_LEFT - PLOT_RIGHT) / rows.len() as f64;
    let slot = if line {
        48.0_f64.max(fit)
    } else {
        (series.len() as f64 * 26.0 + 24.0).max(fit)
    };
    let width = PLOT_LEFT + PLOT_RIGHT + slot * rows.len() as f64;
    let plot_height = CHART_HEIGHT - PLOT_TOP - PLOT_BOTTOM;
    let y = |v: f64| PLOT_TOP + (max - v) / (max - min) * plot_height;
    let x_center = |i: usize| PLOT_LEFT + slot * (i as f64 + 0.5);
    let baseline = y(0.0);
    // Show every label when they fit, otherwise every n-th.
    let label_every = ((70.0 / slot).ceil() as usize).max(1);
    let title = {
        let names = series
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if props.spec.group_by.is_empty() {
            names
        } else {
            format!(
                "{names} by {}",
                props
                    .spec
                    .group_by
                    .iter()
                    .map(|g| field_label(&props.fields, &g.field))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };
    let spread = {
        let peaks: Vec<f64> = series
            .iter()
            .filter_map(|name| {
                rows.iter()
                    .filter_map(|row| value(row, name))
                    .map(f64::abs)
                    .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))))
            })
            .filter(|p| *p > 0.0)
            .collect();
        let biggest = peaks.iter().cloned().fold(0.0, f64::max);
        let smallest = peaks.iter().cloned().fold(f64::INFINITY, f64::min);
        peaks.len() > 1 && biggest / smallest > 20.0
    };

    let bars = |index: usize, row: &SummaryRow| {
        let n = series.len() as f64;
        let thickness = ((slot - 16.0 - 2.0 * (n - 1.0)) / n).clamp(4.0, 24.0);
        let group_width = thickness * n + 2.0 * (n - 1.0);
        let start = x_center(index) - group_width / 2.0;
        html! {<>{for series.iter().enumerate().filter_map(|(s, name)| {
            let v = value(row, name)?;
            let x = start + s as f64 * (thickness + 2.0);
            Some(html!{<path class={format!("mark series-{}", s + 1)} d={bar_path(x, thickness, baseline, y(v))}/>})
        })}</>}
    };
    let lines = || {
        html! {<>{for series.iter().enumerate().map(|(s, name)| {
            // Missing values break the line rather than dropping to zero.
            let mut segments: Vec<Vec<(f64, f64)>> = vec![vec![]];
            for (i, row) in rows.iter().enumerate() {
                match value(row, name) {
                    Some(v) => segments.last_mut().unwrap().push((x_center(i), y(v))),
                    None => segments.push(vec![]),
                }
            }
            html!{<g class={format!("series-{}", s + 1)}>
                {for segments.iter().filter(|seg| seg.len() > 1).map(|seg| html!{
                    <polyline class="line" points={seg.iter().map(|(x, y)| format!("{x:.1},{y:.1}")).collect::<Vec<_>>().join(" ")}/>
                })}
                {for segments.iter().flatten().map(|(x, y)| html!{<circle class="dot" cx={format!("{x:.1}")} cy={format!("{y:.1}")} r="4"/>})}
            </g>}
        })}</>}
    };
    let tooltip = (*hovered).and_then(|i| rows.get(i).map(|row| (i, row))).map(|(i, row)| {
        let left = x_center(i);
        let flip = left > width - 180.0;
        html! {<div class="chart-tooltip" role="status" style={format!("{}:{}px", if flip {"right"} else {"left"}, if flip { width - left + 12.0 } else { left + 12.0 })}>
            <div class="tooltip-title">{row.labels.join(" · ")}</div>
            {for series.iter().enumerate().map(|(s, name)| html!{
                <div class="tooltip-row">
                    <span class={format!("key series-{}", s + 1)} aria-hidden="true"></span>
                    <strong>{format_value(row.values.get(name.as_str()).unwrap_or(&Value::Null))}</strong>
                    <span class="muted">{name.to_string()}</span>
                </div>
            })}
            <div class="tooltip-row muted">{format!("{} records", row.count)}</div>
        </div>}
    });
    html! {<figure class={classes!("summary-chart", if line {"line-chart"} else {"bar-chart"})}>
        <figcaption>
            <strong>{title}</strong>
            if series.len() > 1 {
                <span class="chart-legend">{for series.iter().enumerate().map(|(s, name)| html!{
                    <span class="legend-item"><span class={classes!("key", format!("series-{}", s + 1), line.then_some("line-key"))} aria-hidden="true"></span>{name.to_string()}</span>
                })}</span>
            }
        </figcaption>
        if spread {
            <p class="muted chart-note">{"These values differ a lot in size and share one axis, so the smaller ones look flat. Chart them separately to compare their shapes."}</p>
        }
        <div class="chart-scroll" ref={scroller}>
            <div class="chart-frame" style={format!("width:{width:.0}px")}>
                <svg width={format!("{width:.0}")} height={format!("{CHART_HEIGHT}")} role="img" aria-label={format!("{} chart. The table below lists every value.", if line {"Line"} else {"Bar"})}>
                    {for ticks.iter().map(|t| html!{<g class="tick">
                        <line class={if *t == 0.0 {"axis"} else {"grid"}} x1={format!("{PLOT_LEFT}")} x2={format!("{:.1}", width - PLOT_RIGHT)} y1={format!("{:.1}", y(*t))} y2={format!("{:.1}", y(*t))}/>
                        <text x={format!("{:.1}", PLOT_LEFT - 8.0)} y={format!("{:.1}", y(*t) + 4.0)} text-anchor="end">{compact(*t)}</text>
                    </g>})}
                    if let Some(i) = *hovered {
                        if line {
                            <line class="crosshair" x1={format!("{:.1}", x_center(i))} x2={format!("{:.1}", x_center(i))} y1={format!("{PLOT_TOP}")} y2={format!("{:.1}", CHART_HEIGHT - PLOT_BOTTOM)}/>
                        } else {
                            <rect class="hover-band" x={format!("{:.1}", PLOT_LEFT + slot * i as f64)} y={format!("{PLOT_TOP}")} width={format!("{slot:.1}")} height={format!("{plot_height:.1}")}/>
                        }
                    }
                    if line { {lines()} } else { {for rows.iter().enumerate().map(|(i, row)| bars(i, row))} }
                    {for rows.iter().enumerate().filter(|(i, _)| i % label_every == 0).map(|(i, row)| {
                        let label = row.labels.join(" · ");
                        let short: String = if label.chars().count() > 14 { format!("{}…", label.chars().take(13).collect::<String>()) } else { label.clone() };
                        html!{<text class="x-label" x={format!("{:.1}", x_center(i))} y={format!("{:.1}", CHART_HEIGHT - PLOT_BOTTOM + 18.0)} text-anchor="middle"><title>{label}</title>{short}</text>}
                    })}
                    // Hit areas: a full-height column per group, focusable for keyboard users.
                    {for rows.iter().enumerate().map(|(i, row)| {
                        let (enter, leave) = (
                            { let hovered = hovered.clone(); Callback::from(move |_| hovered.set(Some(i))) },
                            { let hovered = hovered.clone(); Callback::from(move |_| hovered.set(None)) },
                        );
                        let (focus, blur) = (
                            { let hovered = hovered.clone(); Callback::from(move |_: FocusEvent| hovered.set(Some(i))) },
                            { let hovered = hovered.clone(); Callback::from(move |_: FocusEvent| hovered.set(None)) },
                        );
                        let description = format!("{}: {}", row.labels.join(" · "), series.iter().map(|name| format!("{name} {}", format_value(row.values.get(name.as_str()).unwrap_or(&Value::Null)))).collect::<Vec<_>>().join(", "));
                        html!{<rect class="hit" tabindex="0" aria-label={description} x={format!("{:.1}", PLOT_LEFT + slot * i as f64)} y="0" width={format!("{slot:.1}")} height={format!("{CHART_HEIGHT}")} onpointerenter={enter} onpointerleave={leave} onfocus={focus} onblur={blur}/>}
                    })}
                </svg>
                {tooltip}
            </div>
        </div>
    </figure>}
}

/// A bar with a 4px rounded data end and a square end at the baseline.
fn bar_path(x: f64, width: f64, baseline: f64, top: f64) -> String {
    let height = (baseline - top).abs();
    if height < 0.5 {
        return String::new();
    }
    let r = 4.0_f64.min(width / 2.0).min(height);
    if top <= baseline {
        format!(
            "M{x:.1},{baseline:.1}V{:.1}Q{x:.1},{top:.1} {:.1},{top:.1}H{:.1}Q{:.1},{top:.1} {:.1},{:.1}V{baseline:.1}Z",
            top + r,
            x + r,
            x + width - r,
            x + width,
            x + width,
            top + r
        )
    } else {
        format!(
            "M{x:.1},{baseline:.1}V{:.1}Q{x:.1},{top:.1} {:.1},{top:.1}H{:.1}Q{:.1},{top:.1} {:.1},{:.1}V{baseline:.1}Z",
            top - r,
            x + r,
            x + width - r,
            x + width,
            x + width,
            top - r
        )
    }
}

/// Five or so round tick values covering `low..=high`, always including 0.
fn nice_ticks(low: f64, high: f64) -> Vec<f64> {
    let span = (high - low).max(f64::EPSILON);
    let raw = span / 4.0;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|m| m * magnitude)
        .find(|s| span / s <= 5.0)
        .unwrap_or(magnitude * 10.0);
    let start = (low / step).floor() * step;
    let end = (high / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut t = start;
    while t <= end + step / 2.0 {
        ticks.push(if t.abs() < step / 1e6 { 0.0 } else { t });
        t += step;
    }
    if ticks.len() < 2 {
        ticks.push(start + step);
    }
    ticks
}

/// Axis labels: 950, 1.2K, 3M.
fn compact(value: f64) -> String {
    let abs = value.abs();
    let (scaled, suffix) = if abs >= 1e9 {
        (value / 1e9, "B")
    } else if abs >= 1e6 {
        (value / 1e6, "M")
    } else if abs >= 1e3 {
        (value / 1e3, "K")
    } else {
        (value, "")
    };
    let text = format!("{scaled:.1}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_round_and_cover_the_data() {
        assert_eq!(
            nice_ticks(0.0, 4000.0),
            vec![0.0, 1000.0, 2000.0, 3000.0, 4000.0]
        );
        let ticks = nice_ticks(-35.0, 120.0);
        assert!(ticks[0] <= -35.0 && *ticks.last().unwrap() >= 120.0 && ticks.contains(&0.0));
        assert_eq!(compact(1500.0), "1.5K");
        assert_eq!(compact(2_000_000.0), "2M");
        assert_eq!(compact(950.0), "950");
    }

    #[test]
    fn month_buckets_drill_into_real_dates() {
        assert_eq!(
            bucket_range(DateBucket::Month, "2024-02"),
            ("2024-02-01".into(), "2024-02-29".into())
        );
        assert_eq!(
            bucket_range(DateBucket::Month, "2026-04"),
            ("2026-04-01".into(), "2026-04-30".into())
        );
        assert_eq!(
            bucket_range(DateBucket::Year, "2026"),
            ("2026-01-01".into(), "2026-12-31".into())
        );
    }
}
