use serde_json::{Map, Value};
use vulse_model::{
    ArrayItems, Blueprint, CollectionView, DocumentView, FieldDef, FieldType,
    validate_portable_text,
};
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
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
        FieldType::String => Ok(ValueInput::String(expect_string(field, value)?)),
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
        FieldType::String => ValueInput::String(String::new()),
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
}

#[function_component(DocumentForm)]
pub fn document_form(props: &DocumentFormProps) -> Html {
    if props.blueprint.fields.is_empty() {
        return html! { <p class="muted">{"This collection has no fields. Documents are empty objects."}</p> };
    }
    html! {
        <div class="fields card">
            { for props.blueprint.fields.iter().enumerate().map(|(index, field)| {
                if field.kind == FieldType::Calculated {
                    let preview = body_from_inputs(&props.blueprint, &props.inputs).map(|body| vulse_model::calculate_document(&props.blueprint, &body));
                    let result = match preview {
                        Ok((body, errors)) => errors.get(&field.name).cloned().unwrap_or_else(|| vulse_model::text_value(&body[&field.name])),
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
        (FieldType::String, ValueInput::String(value)) => {
            let on_change = props.on_change.clone();
            html! { <input placeholder={props.field.example.clone().unwrap_or_default()} aria-label={props.field.display_label().to_string()} value={value.clone()} oninput={Callback::from(move |event: InputEvent| {
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

#[function_component(RefSelect)]
fn ref_select(props: &RefSelectProps) -> Html {
    let documents = use_state(Vec::<DocumentView>::new);
    let selected = use_state(|| None::<DocumentView>);
    let search = use_state(String::new);
    let offset = use_state(|| 0u32);
    let total = use_state(|| 0u64);
    let loading = use_state(|| false);
    let error = use_state(String::new);
    let epoch = use_mut_ref(|| 0usize);
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
            (target.clone(), (*search).clone(), *offset),
            move |(target, search, offset)| {
                *epoch.borrow_mut() += 1;
                let generation = *epoch.borrow();
                let (target, search, offset) = (target.clone(), search.clone(), *offset);
                loading.set(true);
                let timer = gloo_timers::callback::Timeout::new(200, move || {
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
                                    documents.set(page.items);
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
                selected.set(None);
                let (target, value) = (target.clone(), value.clone());
                wasm_bindgen_futures::spawn_local(async move {
                    if let Some(target) = target {
                        if !value.is_empty() {
                            match api::document(&target.id, &value).await {
                                Ok(d) => selected.set(Some(d)),
                                Err(e) => error.set(e),
                            }
                        }
                    }
                });
            },
        );
    }
    let on_change = props.on_change.clone();
    html! {<div class="reference-picker stack"><input type="search" aria-label={format!("Search {} records",props.collection_name)} placeholder="Search by title or content…" value={(*search).clone()} oninput={Callback::from({let search=search.clone();let offset=offset.clone();move |e:InputEvent|{search.set(e.target_unchecked_into::<HtmlInputElement>().value());offset.set(0);}})}/>
        <select aria-label={format!("Choose {} record",props.collection_name)} value={props.value.clone()} disabled={*loading} onchange={Callback::from(move |e:Event|on_change.emit(ValueInput::Reference(e.target_unchecked_into::<HtmlSelectElement>().value())))}>
            <option value="">{"Select a record"}</option>
            if let Some(d)=selected.as_ref().filter(|d|!documents.iter().any(|r|r.id==d.id)){<option value={d.id.clone()} selected={d.id==props.value}>{document_label(d,target.as_ref().map(|c|&c.blueprint))}</option>}
            {for documents.iter().map(|d|html!{<option value={d.id.clone()} selected={d.id==props.value}>{document_label(d,target.as_ref().map(|c|&c.blueprint))}</option>})}
        </select>
        <div class="row"><span class="muted">{if *loading{"Searching…".into()}else{format!("{} matching records",*total)}}</span><button type="button" class="secondary" disabled={*offset==0||*loading} onclick={Callback::from({let offset=offset.clone();move |_|offset.set(offset.saturating_sub(25))})}>{"Previous matches"}</button><button type="button" class="secondary" disabled={(*offset as u64+25)>=*total||*loading} onclick={Callback::from({let offset=offset.clone();move |_|offset.set(*offset+25)})}>{"More matches"}</button></div>
        if let Some(target)=target{if !props.value.is_empty(){<yew_router::components::Link<crate::app::Route> to={crate::app::Route::Document{id:target.id.clone(),doc_id:props.value.clone()}}>{format!("Open {} →",selected.as_ref().map(|d|vulse_model::record_title(&target.blueprint,&d.body,&d.id)).unwrap_or_else(||"related record".into()))}</yew_router::components::Link<crate::app::Route>>}}
        if !error.is_empty(){<p class="error" role="alert">{(*error).clone()}</p>}
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
