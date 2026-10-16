//! Adds many fields at once from a spreadsheet's column headers.

use crate::api;
use vulse_model::{FieldDef, FieldSuggestions, FieldType};
use web_sys::{File, HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

/// Types a column can be suggested as, with their labels.
const KINDS: [(FieldType, &str, &str); 4] = [
    (FieldType::String, "string", "Text"),
    (FieldType::Number, "number", "Number"),
    (FieldType::Date, "date", "Date"),
    (FieldType::Boolean, "boolean", "Boolean"),
];

#[derive(Clone, PartialEq)]
struct Column {
    header: String,
    samples: Vec<String>,
    /// An existing field already matches the header.
    existing: Option<String>,
    include: bool,
    label: String,
    name: String,
    kind: FieldType,
}

#[derive(Properties, PartialEq)]
pub struct FieldsFromFileProps {
    pub collection_id: String,
    /// The fields in the editor, saved or not.
    pub fields: Vec<FieldDef>,
    /// The chosen fields, to append to the editor's fields.
    pub on_add: Callback<Vec<FieldDef>>,
}

#[function_component(FieldsFromFile)]
pub fn fields_from_file(props: &FieldsFromFileProps) -> Html {
    let open = use_state(|| false);
    let file = use_state(|| None::<File>);
    let found = use_state(|| None::<FieldSuggestions>);
    let columns = use_state(Vec::<Column>::new);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let load = {
        let file = file.clone();
        let found = found.clone();
        let columns = columns.clone();
        let error = error.clone();
        let busy = busy.clone();
        let id = props.collection_id.clone();
        let fields = props.fields.clone();
        Callback::from(move |(chosen, sheet): (Option<File>, String)| {
            let Some(chosen) = chosen else {
                file.set(None);
                found.set(None);
                columns.set(vec![]);
                return;
            };
            file.set(Some(chosen.clone()));
            busy.set(true);
            error.set(String::new());
            let found = found.clone();
            let columns = columns.clone();
            let error = error.clone();
            let busy = busy.clone();
            let id = id.clone();
            let fields = fields.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::suggest_fields(&id, &chosen, &sheet).await {
                    Ok(next) => {
                        columns.set(suggested_columns(&next, &fields));
                        found.set(Some(next));
                    }
                    Err(e) => {
                        error.set(e);
                        found.set(None);
                        columns.set(vec![]);
                    }
                }
                busy.set(false);
            });
        })
    };
    let close = {
        let open = open.clone();
        let file = file.clone();
        let found = found.clone();
        let columns = columns.clone();
        let error = error.clone();
        Callback::from(move |_| {
            open.set(false);
            file.set(None);
            found.set(None);
            columns.set(vec![]);
            error.set(String::new());
        })
    };
    if !*open {
        let open = open.clone();
        return html! {
            <button type="button" class="secondary" onclick={Callback::from(move |_| open.set(true))}>{"Add fields from a spreadsheet…"}</button>
        };
    }
    let problems = problems(&columns, &props.fields);
    let chosen = columns.iter().filter(|c| c.include).count();
    let update = |index: usize, change: fn(&mut Column, String)| {
        let columns = columns.clone();
        move |value: String| {
            let mut next = (*columns).clone();
            change(&mut next[index], value);
            columns.set(next);
        }
    };
    let select_all = |include: bool| {
        let columns = columns.clone();
        Callback::from(move |_| {
            let mut next = (*columns).clone();
            for column in &mut next {
                column.include = include && column.existing.is_none();
            }
            columns.set(next);
        })
    };
    let add = {
        let columns = columns.clone();
        let on_add = props.on_add.clone();
        let close = close.clone();
        Callback::from(move |_| {
            let fields = columns
                .iter()
                .filter(|c| c.include)
                .map(|c| FieldDef {
                    label: Some(c.label.trim().to_string()),
                    description: None,
                    example: None,
                    formula: None,
                    name: c.name.trim().to_string(),
                    required: false,
                    kind: c.kind,
                    collection: None,
                    items: None,
                    options: None,
                    renamed_options: None,
                    renamed_from: None,
                })
                .collect();
            on_add.emit(fields);
            close.emit(());
        })
    };
    html! {
        <section class="fields-from-file">
            <div class="section-heading"><div><h3>{"Add fields from a spreadsheet"}</h3><p class="muted">{"Each column header becomes a field. The type is guessed from the column’s values. Nothing changes until you save the fields."}</p></div><button type="button" class="secondary" onclick={close.reform(|_| ())}>{"Cancel"}</button></div>
            <label class="upload-zone"><strong>{"Choose a .csv or .xlsx file with a header row"}</strong><input type="file" accept=".csv,.xlsx" disabled={*busy} onchange={Callback::from({let load=load.clone();move |e:Event|{let input:HtmlInputElement=e.target_unchecked_into();load.emit((input.files().and_then(|f|f.get(0)),String::new()));}})}/></label>
            if *busy {<p role="status" class="muted">{"Reading columns…"}</p>}
            if !error.is_empty() {<p class="error" role="alert">{(*error).clone()}</p>}
            if let Some(next) = found.as_ref() {
                if next.sheets.len() > 1 {<label>{"Worksheet"}<select aria-label="Worksheet" disabled={*busy} onchange={Callback::from({let load=load.clone();let file=file.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();load.emit(((*file).clone(),input.value()));}})}>{for next.sheets.iter().map(|s|html!{<option value={s.clone()} selected={next.sheet==*s}>{s.clone()}</option>})}</select></label>}
                <div class="row"><span class="muted">{format!("{} columns · {} data rows · {} selected",columns.len(),next.row_count,chosen)}</span><button type="button" class="text-button" onclick={select_all(true)}>{"Select all"}</button><button type="button" class="text-button" onclick={select_all(false)}>{"Select none"}</button></div>
                <div class="table-scroll"><table class="column-picker"><thead><tr><th>{"Add"}</th><th>{"Column"}</th><th>{"Label"}</th><th>{"Field name"}</th><th>{"Type"}</th></tr></thead><tbody>
                {for columns.iter().enumerate().map(|(index,column)|{
                    let problem=problems.get(index).cloned().flatten();
                    let toggle={let columns=columns.clone();Callback::from(move |e:Event|{let mut next=(*columns).clone();next[index].include=e.target_unchecked_into::<HtmlInputElement>().checked();columns.set(next);})};
                    let label=update(index,|c,v|c.label=v);
                    let name=update(index,|c,v|c.name=v);
                    let kind={let columns=columns.clone();Callback::from(move |e:Event|{let mut next=(*columns).clone();let value=e.target_unchecked_into::<HtmlSelectElement>().value();next[index].kind=KINDS.iter().find(|(_,v,_)|*v==value).map_or(FieldType::String,|(k,_,_)|*k);columns.set(next);})};
                    html!{<tr class={if column.include{""}else{"skipped"}}>
                        <td><input type="checkbox" aria-label={format!("Add {}",column.header)} checked={column.include} onchange={toggle}/></td>
                        <td><strong>{column.header.clone()}</strong>
                            if let Some(field)=&column.existing{<div class="muted">{format!("Already a field: {field}")}</div>}
                            else if column.samples.is_empty(){<div class="muted">{"No values"}</div>}
                            else{<div class="muted" title={column.samples.join(" · ")}>{column.samples.join(" · ")}</div>}</td>
                        <td><input aria-label={format!("Label for {}",column.header)} value={column.label.clone()} disabled={!column.include} oninput={Callback::from(move |e:InputEvent|label(e.target_unchecked_into::<HtmlInputElement>().value()))}/></td>
                        <td><input class="code" aria-label={format!("Field name for {}",column.header)} value={column.name.clone()} disabled={!column.include} oninput={Callback::from(move |e:InputEvent|name(e.target_unchecked_into::<HtmlInputElement>().value()))}/>
                            if let Some(problem)=problem{<div class="field-problem" role="alert">{problem}</div>}</td>
                        <td><select aria-label={format!("Type for {}",column.header)} disabled={!column.include} onchange={kind}>{for KINDS.iter().map(|(k,v,l)|html!{<option value={*v} selected={column.kind==*k}>{*l}</option>})}</select></td>
                    </tr>}
                })}
                </tbody></table></div>
                <div class="row"><button type="button" disabled={chosen==0||problems.iter().any(Option::is_some)} onclick={add}>{if chosen==1{"Add 1 field".to_string()}else{format!("Add {chosen} fields")}}</button>
                if problems.iter().any(Option::is_some){<span class="muted">{"Fix the highlighted field names first."}</span>}
                else{<span class="muted">{"Then review them below and choose Save fields."}</span>}</div>
            }
        </section>
    }
}

/// One row per column. Columns an existing field already covers start
/// unchecked; the rest get names unique among all fields.
fn suggested_columns(found: &FieldSuggestions, fields: &[FieldDef]) -> Vec<Column> {
    let headers: Vec<String> = found.columns.iter().map(|c| c.header.clone()).collect();
    let matched = vulse_model::match_headers(&headers, &fields.iter().collect::<Vec<_>>());
    let mut taken: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
    found
        .columns
        .iter()
        .zip(matched)
        .map(|(column, matched)| {
            let existing = matched.map(|i| fields[i].display_label().to_string());
            let name = vulse_model::field_name_from(&column.header, &taken);
            taken.push(name.clone());
            Column {
                header: column.header.clone(),
                samples: column.samples.clone(),
                include: existing.is_none(),
                existing,
                label: column.header.chars().take(200).collect(),
                name,
                kind: column.kind,
            }
        })
        .collect()
}

/// What stops each chosen column from becoming a field.
fn problems(columns: &[Column], fields: &[FieldDef]) -> Vec<Option<String>> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            if !column.include {
                return None;
            }
            let name = column.name.trim();
            if !vulse_model::is_field_name(name) {
                return Some(
                    "Use 1–64 lowercase letters, digits or _, starting with a letter".into(),
                );
            }
            if fields.iter().any(|f| f.name == name)
                || columns[..index]
                    .iter()
                    .any(|c| c.include && c.name.trim() == name)
            {
                return Some("Another field already has this name".into());
            }
            let label = column.label.trim();
            if label.is_empty() || label.chars().count() > 200 {
                return Some("Labels need 1–200 characters".into());
            }
            None
        })
        .collect()
}
