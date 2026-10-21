use serde_json::{Map, Value};
use vulse_model::{
    ArrayItems, Blueprint, CollectionView, DocumentView, FieldDef, FieldType,
    validate_portable_text,
};
use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use yew::prelude::*;

use crate::api;
use crate::editor::RichEditor;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ValueInput {
    Calculated,
    String(String),
    Number(String),
    Boolean(bool),
    Json(String),
    Blocks(Value),
    Asset(String),
    Reference(String),
    List(Vec<ValueInput>),
}

pub fn inputs_from_body(blueprint: &Blueprint, body: &Value) -> Result<Vec<ValueInput>, String> {
    blueprint
        .fields
        .iter()
        .map(|field| input_from_field(field, body.get(&field.name)))
        .collect()
}

pub fn body_from_inputs(blueprint: &Blueprint, inputs: &[ValueInput]) -> Result<Value, String> {
    if blueprint.fields.len() != inputs.len() {
        return Err("the form does not match the blueprint".into());
    }
    let mut body = Map::new();
    for (field, input) in blueprint.fields.iter().zip(inputs) {
        if let Some(value) = value_from_input(field, input)? {
            body.insert(field.name.clone(), value);
        }
    }
    Ok(Value::Object(body))
}

fn input_from_field(field: &FieldDef, value: Option<&Value>) -> Result<ValueInput, String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(blank_input(field));
    };
    match field.kind {
        FieldType::Calculated => Ok(ValueInput::Calculated),
        FieldType::String | FieldType::Date => Ok(ValueInput::String(expect_string(field, value)?)),
        FieldType::Number => {
            if value.is_number() {
                Ok(ValueInput::Number(value.to_string()))
            } else {
                Err(format!("{} must be a number", field.name))
            }
        }
        FieldType::Boolean => {
            Ok(ValueInput::Boolean(value.as_bool().ok_or_else(|| {
                format!("{} must be a boolean", field.name)
            })?))
        }
        FieldType::Json => Ok(ValueInput::Json(
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()),
        )),
        FieldType::PortableText => {
            validate_portable_text(value)?;
            Ok(ValueInput::Blocks(value.clone()))
        }
        FieldType::Asset => Ok(ValueInput::Asset(expect_string(field, value)?)),
        FieldType::Reference => Ok(ValueInput::Reference(expect_string(field, value)?)),
        FieldType::Array => {
            let items = value
                .as_array()
                .ok_or_else(|| format!("{} must be an array", field.name))?;
            items
                .iter()
                .map(|item| input_from_item(field, item))
                .collect::<Result<Vec<_>, _>>()
                .map(ValueInput::List)
        }
    }
}

fn input_from_item(field: &FieldDef, value: &Value) -> Result<ValueInput, String> {
    match field.items {
        Some(ArrayItems::String) => Ok(ValueInput::String(expect_string(field, value)?)),
        Some(ArrayItems::Number) => Ok(ValueInput::Number(value.to_string())),
        Some(ArrayItems::Asset) => Ok(ValueInput::Asset(expect_string(field, value)?)),
        Some(ArrayItems::Reference) => Ok(ValueInput::Reference(expect_string(field, value)?)),
        None => Err(format!("{} is not a list", field.name)),
    }
}

fn blank_input(field: &FieldDef) -> ValueInput {
    match field.kind {
        FieldType::Calculated => ValueInput::Calculated,
        FieldType::String | FieldType::Date => ValueInput::String(String::new()),
        FieldType::Number => ValueInput::Number(String::new()),
        FieldType::Boolean => ValueInput::Boolean(false),
        FieldType::Json => ValueInput::Json(String::new()),
        FieldType::PortableText => ValueInput::Blocks(Value::Array(Vec::new())),
        FieldType::Asset => ValueInput::Asset(String::new()),
        FieldType::Reference => ValueInput::Reference(String::new()),
        FieldType::Array => ValueInput::List(Vec::new()),
    }
}

fn blank_item(field: &FieldDef) -> ValueInput {
    match field.items {
        Some(ArrayItems::Number) => ValueInput::Number(String::new()),
        Some(ArrayItems::Asset) => ValueInput::Asset(String::new()),
        Some(ArrayItems::Reference) => ValueInput::Reference(String::new()),
        _ => ValueInput::String(String::new()),
    }
}

fn value_from_input(field: &FieldDef, input: &ValueInput) -> Result<Option<Value>, String> {
    match (field.kind, input) {
        (FieldType::Calculated, _) => Ok(None),
        (FieldType::String, ValueInput::String(value)) => optional_text(field, value),
        (FieldType::Date, ValueInput::String(value)) => {
            if !value.is_empty() && !vulse_model::is_date(value) {
                return Err(format!("{} must be a date", field.display_label()));
            }
            optional_text(field, value)
        }
        (FieldType::Number, ValueInput::Number(value)) => parse_number(field, value),
        (FieldType::Boolean, ValueInput::Boolean(value)) => Ok(Some(Value::Bool(*value))),
        (FieldType::Json, ValueInput::Json(value)) => parse_json(field, value),
        (FieldType::PortableText, ValueInput::Blocks(blocks)) => {
            if blocks.as_array().is_some_and(Vec::is_empty) && !field.required {
                Ok(None)
            } else {
                Ok(Some(blocks.clone()))
            }
        }
        (FieldType::Asset, ValueInput::Asset(value)) => required_id(field, value),
        (FieldType::Reference, ValueInput::Reference(value)) => required_id(field, value),
        (FieldType::Array, ValueInput::List(items)) => {
            let mut values = Vec::new();
            for item in items {
                let Some(value) = item_value(field, item)? else {
                    return Err(format!("{} has an empty item", field.name));
                };
                values.push(value);
            }
            Ok(Some(Value::Array(values)))
        }
        _ => Err(format!("{} has an unexpected value", field.name)),
    }
}

fn optional_text(field: &FieldDef, value: &str) -> Result<Option<Value>, String> {
    if value.is_empty() && !field.required {
        Ok(None)
    } else if value.is_empty() && field.required {
        Err(format!("{} is required", field.name))
    } else {
        Ok(Some(Value::String(value.to_string())))
    }
}

fn required_id(field: &FieldDef, value: &str) -> Result<Option<Value>, String> {
    if value.is_empty() && !field.required {
        Ok(None)
    } else if value.is_empty() {
        Err(format!("{} is required", field.name))
    } else {
        Ok(Some(Value::String(value.to_string())))
    }
}

fn parse_number(field: &FieldDef, value: &str) -> Result<Option<Value>, String> {
    if value.trim().is_empty() {
        return if field.required {
            Err(format!("{} is required", field.name))
        } else {
            Ok(None)
        };
    }
    let parsed: Value = serde_json::from_str(value.trim())
        .map_err(|_| format!("{} must be a number", field.name))?;
    if parsed.is_number() {
        Ok(Some(parsed))
    } else {
        Err(format!("{} must be a number", field.name))
    }
}

fn parse_json(field: &FieldDef, value: &str) -> Result<Option<Value>, String> {
    if value.trim().is_empty() {
        return if field.required {
            Err(format!("{} is required", field.name))
        } else {
            Ok(None)
        };
    }
    serde_json::from_str(value)
        .map(Some)
        .map_err(|_| format!("{} must be JSON", field.name))
}

fn item_value(field: &FieldDef, input: &ValueInput) -> Result<Option<Value>, String> {
    match (field.items, input) {
        (Some(ArrayItems::String), ValueInput::String(value)) => {
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Value::String(value.clone())))
            }
        }
        (Some(ArrayItems::Number), ValueInput::Number(value)) => parse_number(field, value),
        (Some(ArrayItems::Asset), ValueInput::Asset(value))
        | (Some(ArrayItems::Reference), ValueInput::Reference(value)) => {
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(Value::String(value.clone())))
            }
        }
        _ => Err(format!("{} has an unexpected item", field.name)),
    }
}

fn expect_string(field: &FieldDef, value: &Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{} must be a string", field.name))
}

#[derive(Properties, PartialEq)]
pub struct DocumentFormProps {
    pub collection_id: String,
    pub blueprint: Blueprint,
    pub collections: Vec<CollectionView>,
    pub inputs: Vec<ValueInput>,
    pub on_change: Callback<Vec<ValueInput>>,
    /// The saved record, for formulas that read other collections.
    #[prop_or_default]
    pub saved: Option<serde_json::Value>,
}

/// A calculated result as text, in the field's number format when it has one.
fn calculated_text(field: &FieldDef, value: &Value) -> String {
    if field.number_format.is_some() && value.is_number() {
        crate::number_format::field_display(field, value)
    } else {
        vulse_model::text_value(value)
    }
}

#[function_component(DocumentForm)]
pub fn document_form(props: &DocumentFormProps) -> Html {
    if props.blueprint.fields.is_empty() {
        return html! { <p class="muted">{"This collection has no fields. Documents are empty objects."}</p> };
    }
    html! {
        <div class="fields card">
            { for props.blueprint.fields.iter().enumerate().map(|(index, field)| {
                if field.kind == FieldType::Calculated && !vulse_model::formula_lookups(field.formula.as_deref().unwrap_or_default()).is_empty() {
                    // Other collections' values are only read on the server.
                    let result = match props.saved.as_ref().map(|body| &body[&field.name]) {
                        Some(value) if !value.is_null() => format!("{} · saved value, updates when you save", calculated_text(field, value)),
                        _ => "Calculated from other collections when the record is saved".into(),
                    };
                    return html! { <div class="computed-field"><span class="badge">{"ƒx Calculated"}</span><strong>{field.display_label().to_string()}</strong><output>{result}</output><code>{field.formula.clone().unwrap_or_default()}</code></div> };
                }
                if field.kind == FieldType::Calculated {
                    let preview = body_from_inputs(&props.blueprint, &props.inputs).map(|body| vulse_model::calculate_document(&props.blueprint, &body));
                    let result = match preview {
                        Ok((body, errors)) => errors.get(&field.name).cloned().unwrap_or_else(|| calculated_text(field, &body[&field.name])),
                        Err(_) => "Complete the input fields to see the result".into(),
                    };
                    return html! { <div class="computed-field"><span class="badge">{"ƒx Calculated"}</span><strong>{field.display_label().to_string()}</strong><output>{result}</output><code>{field.formula.clone().unwrap_or_default()}</code></div> };
                }
                let input = props.inputs.get(index).cloned().unwrap_or_else(|| blank_input(field));
                let on_change = {
                    let inputs = props.inputs.clone();
                    let on_change = props.on_change.clone();
                    Callback::from(move |next: ValueInput| {
                        let mut inputs = inputs.clone();
                        if index < inputs.len() {
                            inputs[index] = next;
                            on_change.emit(inputs);
                        }
                    })
                };
                html! {
                    <FieldControl
                        key={field.name.clone()}
                        field={field.clone()}
                        {input}
                        collection_id={props.collection_id.clone()}
                        collections={props.collections.clone()}
                        {on_change}
                    />
                }
            }) }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct FieldControlProps {
    field: FieldDef,
    input: ValueInput,
    collection_id: String,
    collections: Vec<CollectionView>,
    on_change: Callback<ValueInput>,
}

#[function_component(FieldControl)]
fn field_control(props: &FieldControlProps) -> Html {
    html! {
        <div class="field">
            <span>
                {props.field.display_label().to_string()}
                if props.field.required {
                    <span class="required">{" required"}</span>
                }
            </span>
            if let Some(description)=&props.field.description {<p class="field-help">{description.clone()}</p>}
            { control(props) }
            if let Some(example)=&props.field.example {<p class="field-help">{format!("Example: {example}")}</p>}
        </div>
    }
}

fn control(props: &FieldControlProps) -> Html {
    match (&props.field.kind, &props.input) {
        (FieldType::String, ValueInput::String(value)) if props.field.options.is_some() => {
            let on_change = props.on_change.clone();
            html! { <OptionSelect label={props.field.display_label().to_string()} options={props.field.options.clone().unwrap_or_default()} value={value.clone()} on_change={Callback::from(move |value| on_change.emit(ValueInput::String(value)))} /> }
        }
        (FieldType::String, ValueInput::String(value)) => {
            let on_change = props.on_change.clone();
            html! { <input placeholder={props.field.example.clone().unwrap_or_default()} aria-label={props.field.display_label().to_string()} value={value.clone()} oninput={Callback::from(move |event: InputEvent| {
                let element: HtmlInputElement = event.target_unchecked_into();
                on_change.emit(ValueInput::String(element.value()));
            })} /> }
        }
        (FieldType::Date, ValueInput::String(value)) => {
            let on_change = props.on_change.clone();
            html! { <input type="date" aria-label={props.field.display_label().to_string()} value={value.clone()} oninput={Callback::from(move |event: InputEvent| {
                let element: HtmlInputElement = event.target_unchecked_into();
                on_change.emit(ValueInput::String(element.value()));
            })} /> }
        }
        (FieldType::Number, ValueInput::Number(value)) => {
            let on_change = props.on_change.clone();
            html! { <input aria-label={props.field.display_label().to_string()} placeholder={props.field.example.clone().unwrap_or_default()} inputmode="decimal" value={value.clone()} oninput={Callback::from(move |event: InputEvent| {
                let element: HtmlInputElement = event.target_unchecked_into();
                on_change.emit(ValueInput::Number(element.value()));
            })} /> }
        }
        (FieldType::Boolean, ValueInput::Boolean(value)) => {
            let on_change = props.on_change.clone();
            html! { <input aria-label={props.field.display_label().to_string()} type="checkbox" checked={*value} onchange={Callback::from(move |event: Event| {
                let element: HtmlInputElement = event.target_unchecked_into();
                on_change.emit(ValueInput::Boolean(element.checked()));
            })} /> }
        }
        (FieldType::Json, ValueInput::Json(value)) => {
            let on_change = props.on_change.clone();
            html! { <textarea aria-label={props.field.display_label().to_string()} class="code" rows="8" value={value.clone()} oninput={Callback::from(move |event: InputEvent| {
                let element: HtmlTextAreaElement = event.target_unchecked_into();
                on_change.emit(ValueInput::Json(element.value()));
            })} /> }
        }
        (FieldType::PortableText, ValueInput::Blocks(blocks)) => {
            let on_change = props.on_change.clone();
            html! {
                <RichEditor
                    collection_id={props.collection_id.clone()}
                    value={blocks.clone()}
                    label={props.field.display_label().to_string()}
                    on_change={Callback::from(move |blocks| on_change.emit(ValueInput::Blocks(blocks)))}
                />
            }
        }
        (FieldType::Asset, ValueInput::Asset(value)) => html! {
            <AssetControl
                collection_id={props.collection_id.clone()}
                value={value.clone()}
                on_change={props.on_change.clone()}
            />
        },
        (FieldType::Reference, ValueInput::Reference(value)) => html! {
            <RefSelect
                collections={props.collections.clone()}
                collection_name={props.field.collection.clone().unwrap_or_default()}
                value={value.clone()}
                on_change={props.on_change.clone()}
            />
        },
        (FieldType::Array, ValueInput::List(items)) => html! {
            <ArrayControl
                field={props.field.clone()}
                items={items.clone()}
                collection_id={props.collection_id.clone()}
                collections={props.collections.clone()}
                on_change={props.on_change.clone()}
            />
        },
        _ => html! { <p class="error">{"This value does not match the field."}</p> },
    }
}

#[derive(Properties, PartialEq)]
pub struct OptionSelectProps {
    pub label: String,
    pub options: Vec<String>,
    pub value: String,
    pub on_change: Callback<String>,
    #[prop_or_default]
    pub disabled: bool,
}

/// A dropdown of a field's allowed values. The empty choice means no value.
/// A stored value that is no longer allowed stays visible until changed.
#[function_component(OptionSelect)]
pub fn option_select(props: &OptionSelectProps) -> Html {
    let on_change = props.on_change.clone();
    let stale = !props.value.is_empty() && !props.options.contains(&props.value);
    html! {
        <select aria-label={props.label.clone()} disabled={props.disabled} onchange={Callback::from(move |event: Event| {
            let element: web_sys::HtmlSelectElement = event.target_unchecked_into();
            on_change.emit(element.value());
        })}>
            <option value="" selected={props.value.is_empty()}>{"— None —"}</option>
            if stale {
                <option value={props.value.clone()} selected={true}>{format!("{} (not allowed)", props.value)}</option>
            }
            { for props.options.iter().map(|option| html! {
                <option value={option.clone()} selected={*option == props.value}>{option.clone()}</option>
            }) }
        </select>
    }
}

#[derive(Properties, PartialEq)]
struct AssetControlProps {
    collection_id: String,
    value: String,
    on_change: Callback<ValueInput>,
}

#[function_component(AssetControl)]
fn asset_control(props: &AssetControlProps) -> Html {
    let error = use_state(String::new);
    let on_file = {
        let collection_id = props.collection_id.clone();
        let on_change = props.on_change.clone();
        let error = error.clone();
        Callback::from(move |event: Event| {
            let input: HtmlInputElement = event.target_unchecked_into();
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                return;
            };
            let collection_id = collection_id.clone();
            let on_change = on_change.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::upload_asset(&collection_id, &file).await {
                    Ok(asset) => {
                        error.set(String::new());
                        on_change.emit(ValueInput::Asset(asset.id));
                    }
                    Err(err) => error.set(err),
                }
            });
        })
    };
    let clear = {
        let on_change = props.on_change.clone();
        Callback::from(move |_| on_change.emit(ValueInput::Asset(String::new())))
    };
    html! {
        <div class="stack">
            if !props.value.is_empty() {
                <img src={api::asset_content_url(&props.collection_id, &props.value)} alt="" />
                <button type="button" onclick={clear}>{"Clear"}</button>
            }
            <input type="file" onchange={on_file} />
            if !error.is_empty() {
                <p class="error" role="alert">{(*error).clone()}</p>
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct RefSelectProps {
    collections: Vec<CollectionView>,
    collection_name: String,
    value: String,
    on_change: Callback<ValueInput>,
}

/// Searchable picker for one related record. Results load only while the
/// list is open, 25 at a time, so large collections stay fast.
#[function_component(RefSelect)]
fn ref_select(props: &RefSelectProps) -> Html {
    let documents = use_state(Vec::<DocumentView>::new);
    let selected = use_state(|| None::<DocumentView>);
    let search = use_state(String::new);
    let offset = use_state(|| 0u32);
    let total = use_state(|| 0u64);
    let loading = use_state(|| false);
    let error = use_state(String::new);
    let open = use_state(|| false);
    let active = use_state(|| None::<usize>);
    // Shows the search box while a value is set, after "Change".
    let changing = use_state(|| false);
    let input = use_node_ref();
    let epoch = use_mut_ref(|| 0usize);
    let list_id = use_memo((), |_| format!("ref-{}", uuid::Uuid::new_v4().simple()));
    let target = props
        .collections
        .iter()
        .find(|c| c.name == props.collection_name)
        .cloned();
    {
        let documents = documents.clone();
        let total = total.clone();
        let loading = loading.clone();
        let error = error.clone();
        let epoch = epoch.clone();
        use_effect_with(
            (target.clone(), (*search).clone(), *offset, *open),
            move |(target, search, offset, open)| {
                *epoch.borrow_mut() += 1;
                let generation = *epoch.borrow();
                let (target, search, offset) = (target.clone(), search.clone(), *offset);
                let timer = (*open).then(|| {
                    loading.set(true);
                    gloo_timers::callback::Timeout::new(200, move || {
                        wasm_bindgen_futures::spawn_local(async move {
                            if let Some(target) = target {
                                let q = vulse_model::RecordQuery {
                                    search,
                                    offset,
                                    sort_field: "updated_at".into(),
                                    limit: 25,
                                    ..Default::default()
                                };
                                match api::query_records(&target.id, &q).await {
                                    Ok(page) if *epoch.borrow() == generation => {
                                        let mut next = if offset == 0 {
                                            Vec::new()
                                        } else {
                                            (*documents).clone()
                                        };
                                        next.extend(page.items);
                                        documents.set(next);
                                        total.set(page.total);
                                        error.set(String::new());
                                    }
                                    Err(e) if *epoch.borrow() == generation => error.set(e),
                                    _ => {}
                                }
                            } else {
                                documents.set(vec![]);
                                total.set(0);
                                error.set("The referenced collection is unavailable. Reload the collection fields.".into());
                            }
                            if *epoch.borrow() == generation {
                                loading.set(false);
                            }
                        })
                    })
                });
                move || drop(timer)
            },
        );
    }
    {
        let selected = selected.clone();
        let error = error.clone();
        use_effect_with(
            (target.clone(), props.value.clone()),
            move |(target, value)| {
                if selected.as_ref().map(|d| &d.id) != Some(value) {
                    selected.set(None);
                    let (target, value) = (target.clone(), value.clone());
                    wasm_bindgen_futures::spawn_local(async move {
                        if let Some(target) = target
                            && !value.is_empty() {
                                match api::document(&target.id, &value).await {
                                    Ok(d) => selected.set(Some(d)),
                                    Err(e) => error.set(e),
                                }
                            }
                    });
                }
            },
        );
    }
    {
        // Focus the search box when "Change" reveals it.
        let input = input.clone();
        use_effect_with(*changing, move |changing| {
            if *changing
                && let Some(element) = input.cast::<HtmlInputElement>() {
                    let _ = element.focus();
                }
        });
    }
    {
        // Keep the keyboard-highlighted option visible.
        let list_id = (*list_id).clone();
        use_effect_with(*active, move |active| {
            if let Some(index) = active
                && let Some(element) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.get_element_by_id(&format!("{list_id}-{index}")))
                {
                    element.scroll_into_view_with_bool(false);
                }
        });
    }
    let blueprint = target.as_ref().map(|c| c.blueprint.clone());
    let pick = {
        let on_change = props.on_change.clone();
        let (selected, open, active, changing, search, offset) = (
            selected.clone(),
            open.clone(),
            active.clone(),
            changing.clone(),
            search.clone(),
            offset.clone(),
        );
        Callback::from(move |document: DocumentView| {
            on_change.emit(ValueInput::Reference(document.id.clone()));
            selected.set(Some(document));
            open.set(false);
            active.set(None);
            changing.set(false);
            search.set(String::new());
            offset.set(0);
        })
    };
    let close = {
        let (open, active, changing) = (open.clone(), active.clone(), changing.clone());
        Callback::from(move |_: ()| {
            open.set(false);
            active.set(None);
            changing.set(false);
        })
    };
    let name = props.collection_name.clone();
    let list_id = (*list_id).clone();
    let error_line = html! {
        if !error.is_empty() {<p class="error" role="alert">{(*error).clone()}</p>}
    };
    if !props.value.is_empty() && !*changing {
        let label = selected
            .as_ref()
            .map(|d| document_label(d, blueprint.as_ref()))
            .unwrap_or_else(|| "Loading…".into());
        let clear = {
            let on_change = props.on_change.clone();
            let selected = selected.clone();
            Callback::from(move |_| {
                on_change.emit(ValueInput::Reference(String::new()));
                selected.set(None);
            })
        };
        return html! {<div class="reference-picker">
            <div class="reference-selected">
                <span class="reference-value">{label}</span>
                <button type="button" class="secondary" aria-label={format!("Change {name} record")} onclick={Callback::from({let changing=changing.clone();move |_|changing.set(true)})}>{"Change"}</button>
                <button type="button" class="text-button" aria-label={format!("Clear {name} record")} onclick={clear}>{"Clear"}</button>
                if let Some(target)=&target {
                    <yew_router::components::Link<crate::app::Route> to={crate::app::Route::Document{id:target.id.clone(),doc_id:props.value.clone()}}>{format!("Open {} →",selected.as_ref().map(|d|vulse_model::record_title(&target.blueprint,&d.body,&d.id)).unwrap_or_else(||"related record".into()))}</yew_router::components::Link<crate::app::Route>>
                }
            </div>
            {error_line}
        </div>};
    }
    let count = documents.len();
    let onkeydown = {
        let (open, active, documents, pick, close) = (
            open.clone(),
            active.clone(),
            documents.clone(),
            pick.clone(),
            close.clone(),
        );
        Callback::from(move |e: KeyboardEvent| match e.key().as_str() {
            "ArrowDown" | "ArrowUp" => {
                e.prevent_default();
                open.set(true);
                if count > 0 {
                    let next = match (*active, e.key() == "ArrowDown") {
                        (None, true) => 0,
                        (None, false) => count - 1,
                        (Some(i), true) => (i + 1) % count,
                        (Some(i), false) => (i + count - 1) % count,
                    };
                    active.set(Some(next));
                }
            }
            "Enter" if *open => {
                e.prevent_default();
                if let Some(document) = active.and_then(|i| documents.get(i)) {
                    pick.emit(document.clone());
                }
            }
            "Escape" if *open => {
                e.prevent_default();
                close.emit(());
            }
            _ => {}
        })
    };
    let keep_focus = Callback::from(|e: MouseEvent| e.prevent_default());
    html! {<div class="reference-picker combobox">
        <input
            ref={input}
            type="text"
            role="combobox"
            autocomplete="off"
            aria-label={format!("Search {name} records")}
            aria-autocomplete="list"
            aria-expanded={open.to_string()}
            aria-controls={list_id.clone()}
            aria-activedescendant={active.map(|i| format!("{list_id}-{i}"))}
            placeholder={if props.value.is_empty() {format!("Search {name}…")} else {"Search for a replacement…".into()}}
            value={(*search).clone()}
            onfocus={Callback::from({let open=open.clone();move |_|open.set(true)})}
            onclick={Callback::from({let open=open.clone();move |_|open.set(true)})}
            onblur={Callback::from({let close=close.clone();move |_|close.emit(())})}
            oninput={Callback::from({let (search,offset,open,active)=(search.clone(),offset.clone(),open.clone(),active.clone());move |e:InputEvent|{search.set(e.target_unchecked_into::<HtmlInputElement>().value());offset.set(0);open.set(true);active.set(None);}})}
            {onkeydown}
        />
        if *open {
            <ul class="combobox-list" role="listbox" id={list_id.clone()} aria-label={format!("{name} records")}>
                if documents.is_empty() {
                    <li class="combobox-note" role="presentation">{if *loading {"Searching…"} else {"No matching records"}}</li>
                }
                { for documents.iter().enumerate().map(|(index, document)| {
                    let title = blueprint.as_ref().map(|b| vulse_model::record_title(b, &document.body, &document.id)).unwrap_or_else(|| document.id.clone());
                    let choose = {let pick=pick.clone();let document=document.clone();Callback::from(move |e:MouseEvent|{e.prevent_default();pick.emit(document.clone());})};
                    html! {<li id={format!("{list_id}-{index}")} role="option" aria-selected={(*active==Some(index)).to_string()} class={classes!((document.id==props.value).then_some("current"), (*active==Some(index)).then_some("active"))} onmousedown={choose}>
                        {title}<span class="muted">{format!(" · {}", document.id.chars().take(8).collect::<String>())}</span>
                    </li>}
                }) }
                if (count as u64) < *total {
                    <li class="combobox-more" role="presentation">
                        <button type="button" tabindex="-1" class="text-button" disabled={*loading} onmousedown={keep_focus.clone()} onclick={Callback::from({let offset=offset.clone();move |_|offset.set(*offset+25)})}>
                            {if *loading {"Loading…".to_string()} else {format!("Show more ({} more)", *total - count as u64)}}
                        </button>
                    </li>
                }
            </ul>
        }
        if *changing {
            <button type="button" class="text-button" onmousedown={keep_focus} onclick={Callback::from({let close=close.clone();move |_|close.emit(())})}>{"Keep current record"}</button>
        }
        {error_line}
    </div>}
}

#[derive(Properties, PartialEq)]
struct ArrayControlProps {
    field: FieldDef,
    items: Vec<ValueInput>,
    collection_id: String,
    collections: Vec<CollectionView>,
    on_change: Callback<ValueInput>,
}

#[function_component(ArrayControl)]
fn array_control(props: &ArrayControlProps) -> Html {
    let add = {
        let items = props.items.clone();
        let field = props.field.clone();
        let on_change = props.on_change.clone();
        Callback::from(move |_| {
            let mut items = items.clone();
            items.push(blank_item(&field));
            on_change.emit(ValueInput::List(items));
        })
    };
    html! {
        <div class="stack">
            { for props.items.iter().enumerate().map(|(index, item)| {
                let remove = {
                    let items = props.items.clone();
                    let on_change = props.on_change.clone();
                    Callback::from(move |_| {
                        let mut items = items.clone();
                        if index < items.len() {
                            items.remove(index);
                            on_change.emit(ValueInput::List(items));
                        }
                    })
                };
                let on_item = {
                    let items = props.items.clone();
                    let on_change = props.on_change.clone();
                    Callback::from(move |next: ValueInput| {
                        let mut items = items.clone();
                        if index < items.len() {
                            items[index] = next;
                            on_change.emit(ValueInput::List(items));
                        }
                    })
                };
                html! {
                    <div class="row" key={index}>
                        <ItemControl
                            field={props.field.clone()}
                            input={item.clone()}
                            collection_id={props.collection_id.clone()}
                            collections={props.collections.clone()}
                            on_change={on_item}
                        />
                        <button type="button" class="danger" onclick={remove}>{"Remove"}</button>
                    </div>
                }
            }) }
            <button type="button" onclick={add}>{"Add item"}</button>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct ItemControlProps {
    field: FieldDef,
    input: ValueInput,
    collection_id: String,
    collections: Vec<CollectionView>,
    on_change: Callback<ValueInput>,
}

#[function_component(ItemControl)]
fn item_control(props: &ItemControlProps) -> Html {
    let mut field = props.field.clone();
    field.kind = match field.items {
        Some(ArrayItems::Number) => FieldType::Number,
        Some(ArrayItems::Asset) => FieldType::Asset,
        Some(ArrayItems::Reference) => FieldType::Reference,
        _ => FieldType::String,
    };
    field.required = true;
    html! {
        <FieldControl
            {field}
            input={props.input.clone()}
            collection_id={props.collection_id.clone()}
            collections={props.collections.clone()}
            on_change={props.on_change.clone()}
        />
    }
}

pub fn document_label(
    document: &DocumentView,
    blueprint: Option<&vulse_model::Blueprint>,
) -> String {
    blueprint
        .map(|blueprint| {
            format!(
                "{} · {}",
                vulse_model::record_title(blueprint, &document.body, &document.id),
                document.id.chars().take(8).collect::<String>()
            )
        })
        .unwrap_or_else(|| document.id.clone())
}
