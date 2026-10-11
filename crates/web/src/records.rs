use crate::{api, app::Route};
use vulse_model::{
    CollectionView, DocumentView, FieldType, RecordFilter, RecordPage, RecordQuery, SavedView,
    SavedViewWrite, display_value, record_title,
};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::*;

#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen::prelude::wasm_bindgen(catch,js_namespace=["window","VulseEditor"],js_name=download)]
    fn download(name: &str, bytes: &[u8], mime: &str) -> Result<(), wasm_bindgen::JsValue>;
}

#[derive(Properties, PartialEq)]
pub struct RecordsProps {
    pub collection: CollectionView,
    pub collections: Vec<CollectionView>,
}
#[function_component(RecordsPanel)]
pub fn records_panel(props: &RecordsProps) -> Html {
    let query = use_state(RecordQuery::default);
    let page = use_state(|| RecordPage {
        items: vec![],
        total: 0,
        limit: 25,
        offset: 0,
    });
    let columns = use_state(|| {
        props
            .collection
            .blueprint
            .fields
            .iter()
            .map(|f| f.name.clone())
            .collect::<Vec<_>>()
    });
    let views = use_state(Vec::<SavedView>::new);
    let selected = use_state(String::new);
    let view_name = use_state(String::new);
    let loading = use_state(|| true);
    let error = use_state(String::new);
    let status = use_state(String::new);
    let reload = use_state(|| 0usize);
    let query_error = use_state(String::new);
    let epoch = use_mut_ref(|| 0usize);
    let picked = use_state(Vec::<DocumentView>::new);
    let bulk_open = use_state(|| false);
    let bulk_field = use_state(|| {
        props
            .collection
            .blueprint
            .fields
            .iter()
            .find(|f| {
                matches!(
                    f.kind,
                    FieldType::String | FieldType::Number | FieldType::Boolean
                )
            })
            .map(|f| f.name.clone())
            .unwrap_or_default()
    });
    let bulk_value = use_state(String::new);
    let working = use_state(|| false);
    let options_open = use_state(|| false);
    let export_open = use_state(|| false);
    let export_format = use_state(|| "csv".to_string());
    let last_undo = use_state(|| None::<String>);
    let serial = use_mut_ref(|| 0usize);
    let refresh = Callback::from({
        let reload = reload.clone();
        let loading = loading.clone();
        let serial = serial.clone();
        move |()| {
            loading.set(true);
            *serial.borrow_mut() += 1;
            reload.set(*serial.borrow());
        }
    });
    {
        let picked = picked.clone();
        use_effect_with((*query).clone(), move |_| picked.set(vec![]));
    }
    let on_saved = Callback::from({
        let refresh = refresh.clone();
        let undo = last_undo.clone();
        let status = status.clone();
        let error = error.clone();
        move |result: vulse_model::MutationResult| {
            undo.set(if result.undo_id.is_empty() {
                None
            } else {
                Some(result.undo_id)
            });
            status.set(format!("Saved changes to {} record(s)", result.affected));
            error.set(String::new());
            refresh.emit(());
        }
    });
    let run_bulk = Callback::from({
        let picked = picked.clone();
        let working = working.clone();
        let field = bulk_field.clone();
        let value = bulk_value.clone();
        let collection = props.collection.clone();
        let error = error.clone();
        let saved = on_saved.clone();
        let bulk_open = bulk_open.clone();
        move |action: String| {
            if *working || picked.is_empty() {
                return;
            }
            if action == "delete"
                && !crate::app::confirm(&format!(
                    "Delete {} selected records? You can undo this action.",
                    picked.len()
                ))
            {
                return;
            }
            let patch = if action == "update" {
                let Some(def) = collection
                    .blueprint
                    .fields
                    .iter()
                    .find(|f| f.name == *field)
                else {
                    return;
                };
                let parsed = match def.kind {
                    FieldType::Number => {
                        if value.trim().is_empty() {
                            serde_json::Value::Null
                        } else {
                            match serde_json::from_str::<serde_json::Value>(&value) {
                                Ok(value) if value.is_number() => value,
                                _ => {
                                    error.set("Enter a valid number".into());
                                    return;
                                }
                            }
                        }
                    }
                    FieldType::Boolean => serde_json::json!(value.eq_ignore_ascii_case("true")),
                    _ => serde_json::json!((*value).clone()),
                };
                serde_json::json!({def.name.clone():parsed})
            } else {
                serde_json::Value::Null
            };
            let body = vulse_model::BulkWrite {
                action,
                ids: picked.iter().map(|d| d.id.clone()).collect(),
                patch,
                expected: picked
                    .iter()
                    .map(|d| (d.id.clone(), d.updated_at.clone()))
                    .collect(),
            };
            let id = collection.id.clone();
            let picked = picked.clone();
            let working = working.clone();
            let error = error.clone();
            let saved = saved.clone();
            let bulk_open = bulk_open.clone();
            working.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::bulk_records(&id, &body).await {
                    Ok(result) => {
                        picked.set(vec![]);
                        bulk_open.set(false);
                        saved.emit(result);
                    }
                    Err(message) => error.set(message),
                }
                working.set(false);
            });
        }
    });
    let undo = Callback::from({
        let id = props.collection.id.clone();
        let action = last_undo.clone();
        let working = working.clone();
        let error = error.clone();
        let saved = on_saved.clone();
        move |_| {
            if *working {
                return;
            }
            let Some(action) = (*action).clone() else {
                return;
            };
            let id = id.clone();
            let working = working.clone();
            let error = error.clone();
            let saved = saved.clone();
            working.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::undo_action(&id, &action).await {
                    Ok(result) => saved.emit(result),
                    Err(message) => error.set(message),
                }
                working.set(false);
            });
        }
    });

    {
        let page = page.clone();
        let error = query_error.clone();
        let loading = loading.clone();
        let epoch = epoch.clone();
        let query_state = query.clone();
        use_effect_with(
            (props.collection.clone(), (*query).clone(), *reload),
            move |(collection, query, _)| {
                loading.set(true);
                *epoch.borrow_mut() += 1;
                let current = *epoch.borrow();
                let id = collection.id.clone();
                let query = query.clone();
                let timer = gloo_timers::callback::Timeout::new(200, move || {
                    loading.set(true);
                    wasm_bindgen_futures::spawn_local(async move {
                        let result = api::query_records(&id, &query).await;
                        if *epoch.borrow() != current {
                            return;
                        }
                        match result {
                            Ok(next) => {
                                if query.offset > 0 && query.offset as u64 >= next.total {
                                    let mut corrected = query.clone();
                                    corrected.offset = if next.total == 0 {
                                        0
                                    } else {
                                        ((next.total - 1) / query.limit as u64) as u32 * query.limit
                                    };
                                    query_state.set(corrected);
                                }
                                page.set(next);
                                error.set(String::new());
                            }
                            Err(message) => error.set(message),
                        }
                        loading.set(false);
                    });
                });
                move || drop(timer)
            },
        );
    }
    {
        let views = views.clone();
        let error = error.clone();
        use_effect_with(props.collection.id.clone(), move |id| {
            let id = id.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::views(&id).await {
                    Ok(next) => views.set(next),
                    Err(message) => error.set(message),
                }
            });
        });
    }
    {
        let refresh = refresh.clone();
        use_effect_with(props.collection.id.clone(), move |_| {
            let timer = gloo_timers::callback::Interval::new(15_000, move || {
                refresh.emit(());
            });
            move || drop(timer)
        });
    }
    let change = Callback::from({
        let query = query.clone();
        move |mut next: RecordQuery| {
            next.offset = 0;
            query.set(next);
        }
    });
    let save_view = Callback::from({
        let id = props.collection.id.clone();
        let query = query.clone();
        let columns = columns.clone();
        let selected = selected.clone();
        let view_name = view_name.clone();
        let views = views.clone();
        let error = error.clone();
        let status = status.clone();
        move |_| {
            let config = SavedViewWrite {
                name: (*view_name).clone(),
                query: (*query).clone(),
                columns: (*columns).clone(),
            };
            let view = if selected.is_empty() {
                None
            } else {
                Some((*selected).clone())
            };
            let id = id.clone();
            let views = views.clone();
            let selected = selected.clone();
            let error = error.clone();
            let status = status.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::save_view(&id, view.as_deref(), &config).await {
                    Ok(saved) => {
                        selected.set(saved.id);
                        if let Ok(next) = api::views(&id).await {
                            views.set(next);
                        }
                        error.set(String::new());
                        status.set("View saved".into());
                    }
                    Err(message) => error.set(message),
                }
            });
        }
    });
    let remove_view = Callback::from({
        let id = props.collection.id.clone();
        let selected = selected.clone();
        let views = views.clone();
        let query = query.clone();
        let view_name = view_name.clone();
        let error = error.clone();
        move |_| {
            if !crate::app::confirm("Delete this saved view?") {
                return;
            }
            let id = id.clone();
            let view = (*selected).clone();
            let selected = selected.clone();
            let views = views.clone();
            let query = query.clone();
            let view_name = view_name.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::delete_view(&id, &view).await {
                    Ok(()) => {
                        selected.set(String::new());
                        view_name.set(String::new());
                        query.set(RecordQuery::default());
                        if let Ok(next) = api::views(&id).await {
                            views.set(next);
                        }
                    }
                    Err(message) => error.set(message),
                }
            });
        }
    });
    let run_export = Callback::from({
        let query = query.clone();
        let columns = columns.clone();
        let picked = picked.clone();
        let format = export_format.clone();
        let working = working.clone();
        let error = error.clone();
        let status = status.clone();
        let collection = props.collection.clone();
        move |selected_only: bool| {
            if *working || columns.is_empty() || selected_only && picked.is_empty() {
                return;
            }
            let write = vulse_model::ExportRequest {
                query: (*query).clone(),
                columns: (*columns).clone(),
                format: (*format).clone(),
                ids: if selected_only {
                    picked.iter().map(|d| d.id.clone()).collect()
                } else {
                    vec![]
                },
            };
            working.set(true);
            error.set(String::new());
            let (working, error, status, collection) = (
                working.clone(),
                error.clone(),
                status.clone(),
                collection.clone(),
            );
            wasm_bindgen_futures::spawn_local(async move {
                match api::export_records(&collection.id, &write).await {
                    Ok(bytes) => {
                        if download(
                            &format!("{}.{}", collection.name, write.format),
                            &bytes,
                            if write.format == "csv" {
                                "text/csv"
                            } else {
                                "application/json"
                            },
                        )
                        .is_ok()
                        {
                            status.set("Export downloaded".into());
                        } else {
                            error.set("Could not download the export. Please try again.".into());
                        }
                    }
                    Err(e) => error.set(e),
                }
                working.set(false);
            });
        }
    });
    let fields = &props.collection.blueprint.fields;
    let visible: Vec<_> = fields
        .iter()
        .filter(|f| columns.contains(&f.name))
        .collect();
    let pages = page.total.div_ceil(query.limit as u64).max(1);
    let current = query.offset as u64 / query.limit as u64;
    html! {<section class="card data-card">
            <div class="view-toolbar"><label>{"Collection view"}<select aria-label="Collection view" onchange={Callback::from({let views=views.clone();let selected=selected.clone();let query=query.clone();let columns=columns.clone();let view_name=view_name.clone();let all=fields.iter().map(|f|f.name.clone()).collect::<Vec<_>>();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();let id=input.value();if let Some(view)=views.iter().find(|v|v.id==id){query.set(view.config.query.clone());columns.set(view.config.columns.clone());view_name.set(view.config.name.clone());}else{query.set(RecordQuery::default());columns.set(all.clone());view_name.set(String::new());}selected.set(id);}})}><option value="" selected={selected.is_empty()}>{"All records"}</option>{for views.iter().map(|view|html!{<option value={view.id.clone()} selected={*selected==view.id}>{view.config.name.clone()}</option>})}</select></label></div>
    <div class="records-search"><label>{"Search records"}<input type="search" aria-label="Search records" placeholder="Search record values…" value={query.search.clone()} oninput={Callback::from({let query=(*query).clone();let change=change.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();let mut next=query.clone();next.search=input.value();change.emit(next);}})}/></label></div>
    <details class="view-options" open={*options_open} ontoggle={Callback::from({let open=options_open.clone();move |e:Event|open.set(e.target_unchecked_into::<web_sys::Element>().has_attribute("open"))})}><summary>{"Customize view"}<span class="muted">{if query.filters.is_empty(){"Filters · sort · columns · save".into()}else{format!("{} active filter(s) · sort · columns · save",query.filters.len())}}</span></summary><div class="view-toolbar">
            <label>{"View name"}<input aria-label="View name" maxlength="100" placeholder="e.g. Active products" value={(*view_name).clone()} oninput={Callback::from({let name=view_name.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();name.set(input.value());}})}/></label><button type="button" class="secondary" disabled={view_name.trim().is_empty()} onclick={save_view}>{"Save view"}</button>
            if !selected.is_empty(){<button type="button" class="secondary" onclick={Callback::from({let selected=selected.clone();let name=view_name.clone();move |_|{selected.set(String::new());name.set(String::new());}})}>{"Save as new"}</button><button type="button" class="danger" onclick={remove_view}>{"Delete view"}</button>}
            </div>
    <div class="records-controls">
            <label>{"Sort by"}<select aria-label="Sort by" onchange={Callback::from({let query=(*query).clone();let change=change.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();let mut next=query.clone();next.sort_field=input.value();change.emit(next);}})}>{for [("created_at","Created"),("updated_at","Updated"),("id","Record ID")].into_iter().map(|(key,label)|html!{<option value={key} selected={query.sort_field==key}>{label}</option>})}{for fields.iter().map(|f|html!{<option value={f.name.clone()} selected={query.sort_field==f.name}>{f.display_label().to_string()}</option>})}</select></label>
            <label>{"Order"}<select aria-label="Sort order" onchange={Callback::from({let query=(*query).clone();let change=change.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();let mut next=query.clone();next.descending=input.value()=="desc";change.emit(next);}})}><option value="asc" selected={!query.descending}>{"Ascending"}</option><option value="desc" selected={query.descending}>{"Descending"}</option></select></label>
            <label>{"Group by"}<select aria-label="Group by" onchange={Callback::from({let query=(*query).clone();let change=change.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();let mut next=query.clone();next.group_field=input.value();change.emit(next);}})}><option value="" selected={query.group_field.is_empty()}>{"No grouping"}</option>{for fields.iter().map(|f|html!{<option value={f.name.clone()} selected={query.group_field==f.name}>{f.display_label().to_string()}</option>})}</select></label></div>
            <div class="record-filters">{for query.filters.iter().enumerate().map(|(index,filter)|{
                let patch=Callback::from({let query=(*query).clone();let change=change.clone();move |next:RecordFilter|{let mut query=query.clone();query.filters[index]=next;change.emit(query);}});
                let numeric=fields.iter().find(|f|f.name==filter.field).is_some_and(|f|f.kind==FieldType::Number)||["gt","gte","lt","lte"].contains(&filter.op.as_str());
                let boolean=fields.iter().find(|f|f.name==filter.field).is_some_and(|f|f.kind==FieldType::Boolean)&&["eq","neq"].contains(&filter.op.as_str());
                html!{<div class="record-filter"><select aria-label={format!("Filter {} field",index+1)} onchange={Callback::from({let filter=filter.clone();let patch=patch.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();let mut next=filter.clone();next.field=input.value();next.op="contains".into();next.value=serde_json::json!("");patch.emit(next);}})}>{for fields.iter().map(|f|html!{<option value={f.name.clone()} selected={filter.field==f.name}>{f.display_label().to_string()}</option>})}</select>
                    <select aria-label={format!("Filter {} operator",index+1)} onchange={Callback::from({let filter=filter.clone();let patch=patch.clone();let kind=fields.iter().find(|f|f.name==filter.field).map(|f|f.kind);move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();let mut next=filter.clone();next.op=input.value();if ["gt","gte","lt","lte"].contains(&next.op.as_str())||kind==Some(FieldType::Number){next.value=serde_json::json!(0);}else if kind==Some(FieldType::Boolean){next.value=serde_json::json!(true);}patch.emit(next);}})}>{for [("contains","Contains"),("eq","Equals"),("neq","Does not equal"),("gt","Greater than"),("gte","At least"),("lt","Less than"),("lte","At most"),("empty","Is empty"),("not_empty","Is not empty")].into_iter().filter(|(op,_)|!["gt","gte","lt","lte"].contains(op)||fields.iter().find(|f|f.name==filter.field).is_some_and(|f|matches!(f.kind,FieldType::Number|FieldType::Calculated))).map(|(op,label)|html!{<option value={op} selected={filter.op==op}>{label}</option>})}</select>
                    if !["empty","not_empty"].contains(&filter.op.as_str()){if boolean {<select aria-label={format!("Filter {} value",index+1)} onchange={Callback::from({let filter=filter.clone();let patch=patch.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();let mut next=filter.clone();next.value=serde_json::json!(input.value()=="true");patch.emit(next);}})}><option value="true" selected={filter.value==true}>{"True"}</option><option value="false" selected={filter.value==false}>{"False"}</option></select>}else{<input aria-label={format!("Filter {} value",index+1)} type={if numeric&&filter.op!="contains"{"number"}else{"text"}} step="any" value={display_value(&filter.value)} oninput={Callback::from({let filter=filter.clone();let patch=patch.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();let mut next=filter.clone();next.value=if numeric&&next.op!="contains"{serde_json::from_str(&input.value()).unwrap_or(serde_json::Value::String(input.value()))}else{serde_json::Value::String(input.value())};patch.emit(next);}})}/>}}
                    <button type="button" class="secondary" aria-label={format!("Remove filter {}",index+1)} onclick={Callback::from({let query=(*query).clone();let change=change.clone();move |_|{let mut next=query.clone();next.filters.remove(index);change.emit(next);}})}>{"Remove"}</button></div>}
            })}<div class="row"><button type="button" class="secondary" disabled={fields.is_empty()||query.filters.len()>=20} onclick={Callback::from({let fields=fields.clone();let query=(*query).clone();let change=change.clone();move |_|{if let Some(field)=fields.first(){let mut next=query.clone();next.filters.push(RecordFilter{field:field.name.clone(),op:"contains".into(),value:serde_json::json!("")});change.emit(next);}}})}>{"+ Add filter"}</button>
            <details class="column-picker"><summary>{"Visible columns"}</summary>{for fields.iter().map(|f|{let name=f.name.clone();let columns=columns.clone();html!{<label class="check"><input type="checkbox" checked={columns.contains(&name)} onchange={Callback::from({let name=name.clone();move |e:Event|{let input:HtmlInputElement=e.target_unchecked_into();let mut next=(*columns).clone();if input.checked(){next.push(name.clone());}else{next.retain(|item|item!=&name);}columns.set(next);}})}/>{f.display_label().to_string()}</label>}})}</details></div></div>
            if !query_error.is_empty(){<p class="error" role="alert">{(*query_error).clone()}</p>}
    </details>
            if !error.is_empty(){<p class="error" role="alert">{(*error).clone()}</p>}if !status.is_empty(){<p class="success" role="status">{(*status).clone()}</p>}
            if last_undo.is_some(){<div class="undo-banner"><span>{"The last table action can be undone."}</span><button type="button" class="secondary" disabled={*working} onclick={undo}>{"Undo last action"}</button></div>}
            <details class="export-options" open={*export_open} ontoggle={Callback::from({let open=export_open.clone();move |e:Event|open.set(e.target_unchecked_into::<web_sys::Element>().has_attribute("open"))})}><summary>{format!("Export records · {}",export_format.to_uppercase())}</summary>        <div class="export-toolbar row"><label>{"Export format"}<select aria-label="Export format" onchange={Callback::from({let format=export_format.clone();move |e:Event|format.set(e.target_unchecked_into::<HtmlSelectElement>().value())})}><option value="csv" selected={*export_format=="csv"}>{"CSV spreadsheet"}</option><option value="json" selected={*export_format=="json"}>{"JSON with record IDs"}</option></select></label><button type="button" class="secondary" disabled={*loading||*working||columns.is_empty()} onclick={Callback::from({let export=run_export.clone();move |_|export.emit(false)})}>{"Export current view"}</button><span class="muted">{"All matching records and visible fields, including other pages."}</span></div></details>
            if !picked.is_empty(){<div class="bulk-toolbar"><button type="button" class="secondary" disabled={*working||columns.is_empty()} onclick={Callback::from({let export=run_export.clone();move |_|export.emit(true)})}>{"Export selected"}</button><strong>{format!("{} selected",picked.len())}</strong><button type="button" class="secondary" disabled={*working||bulk_field.is_empty()} onclick={Callback::from({let open=bulk_open.clone();move |_|open.set(!*open)})}>{"Bulk edit"}</button><button type="button" class="secondary" disabled={*working} onclick={Callback::from({let run=run_bulk.clone();move |_|run.emit("duplicate".into())})}>{"Duplicate selected"}</button><button type="button" class="danger" disabled={*working} onclick={Callback::from({let run=run_bulk.clone();move |_|run.emit("delete".into())})}>{"Delete selected"}</button><button type="button" class="text-button" onclick={Callback::from({let picked=picked.clone();move |_|picked.set(vec![])})}>{"Clear selection"}</button></div>}
            if *bulk_open&&!picked.is_empty(){<div class="bulk-editor"><label>{"Field to update"}<select aria-label="Bulk field" onchange={Callback::from({let field=bulk_field.clone();let value=bulk_value.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();field.set(input.value());value.set(String::new());}})}>{for fields.iter().filter(|f|matches!(f.kind,FieldType::String|FieldType::Number|FieldType::Boolean)).map(|f|html!{<option value={f.name.clone()} selected={*bulk_field==f.name}>{f.display_label().to_string()}</option>})}</select></label><label>{"New value"}{if fields.iter().find(|f|f.name==*bulk_field).is_some_and(|f|f.kind==FieldType::Boolean){html!{<select aria-label="Bulk value" onchange={Callback::from({let value=bulk_value.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();value.set(input.value());}})}><option value="false" selected={!bulk_value.eq_ignore_ascii_case("true")}>{"False"}</option><option value="true" selected={bulk_value.eq_ignore_ascii_case("true")}>{"True"}</option></select>}}else{html!{<input aria-label="Bulk value" value={(*bulk_value).clone()} oninput={Callback::from({let value=bulk_value.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();value.set(input.value());}})}/>}}}</label><button type="button" disabled={*working} onclick={Callback::from({let run=run_bulk.clone();move |_|run.emit("update".into())})}>{"Apply to selected"}</button><p class="muted">{"Only this field changes. Calculated fields stay read-only. The whole batch must pass validation."}</p></div>}
            <div class="table-toolbar"><h2>{if selected.is_empty(){"All records".into()}else{(*view_name).clone()}}</h2><span class="muted" role="status">{if *loading{"Loading…".into()}else{format!("{} matching records",page.total)}}</span></div>
            if page.items.is_empty() && !*loading{<div class="empty-state"><h2>{if page.total==0&&query.filters.is_empty()&&query.search.is_empty(){"No records yet"}else{"No matching records"}}</h2><p>{"Add a record, import data, or adjust your view."}</p></div>}
            else{<div class="table-scroll"><table class="records-table"><thead><tr><th><input type="checkbox" aria-label="Select visible records" disabled={*loading||*working} checked={!page.items.is_empty()&&page.items.iter().all(|d|picked.iter().any(|p|p.id==d.id))} onchange={Callback::from({let picked=picked.clone();let items=page.items.clone();move |e:Event|{let input:HtmlInputElement=e.target_unchecked_into();picked.set(if input.checked(){items.clone()}else{vec![]});}})}/></th><th>{"Record"}</th>{for visible.iter().map(|f|html!{<th>{if f.kind==FieldType::Calculated{"ƒx "}else{""}}{f.display_label().to_string()}</th>})}<th>{"Updated"}</th></tr></thead><tbody>{for page.items.iter().enumerate().map(|(index,document)|{
                let group=if query.group_field.is_empty(){None}else{let value=display_value(&document.body[&query.group_field]);if index==0||display_value(&page.items[index-1].body[&query.group_field])!=value{Some(if value.is_empty(){"Empty".into()}else{value})}else{None}};
                html!{<>{if let Some(group)=group{html!{<tr class="group-row"><th colspan={(visible.len()+3).to_string()}>{group}</th></tr>}}else{html!{}}}<tr key={document.id.clone()}><td><input type="checkbox" aria-label={format!("Select record {}",document.id)} disabled={*loading||*working} checked={picked.iter().any(|p|p.id==document.id)} onchange={Callback::from({let picked=picked.clone();let document=document.clone();move |e:Event|{let input:HtmlInputElement=e.target_unchecked_into();let mut next=(*picked).clone();next.retain(|d|d.id!=document.id);if input.checked(){next.push(document.clone());}picked.set(next);}})}/></td><td><Link<Route> to={Route::Document{id:props.collection.id.clone(),doc_id:document.id.clone()}}>{format!("{} →",record_title(&props.collection.blueprint,&document.body,&document.id))}</Link<Route>></td>{for visible.iter().map(|field|html!{<crate::record_cell::RecordCell key={field.name.clone()} collections={props.collections.clone()} collection_id={props.collection.id.clone()} document={document.clone()} field={(*field).clone()} on_saved={on_saved.clone()}/>})}<td class="muted">{document.updated_at.chars().take(10).collect::<String>()}</td></tr></>}
            })}</tbody></table></div>}
            <div class="table-footer"><span class="muted">{format!("{} records · Page {} of {}",page.total,current+1,pages)}</span><div class="row"><label class="page-size">{"Rows"}<select aria-label="Rows per page" onchange={Callback::from({let query=(*query).clone();let change=change.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();let mut next=query.clone();next.limit=input.value().parse().unwrap_or(25);change.emit(next);}})}>{for [25,50,100].into_iter().map(|size|html!{<option value={size.to_string()} selected={query.limit==size}>{size}</option>})}</select></label><button type="button" class="secondary" disabled={query.offset==0||*loading} onclick={Callback::from({let query=query.clone();move |_|{let mut next=(*query).clone();next.offset=next.offset.saturating_sub(next.limit);query.set(next);}})}>{"Previous"}</button><button type="button" class="secondary" disabled={current+1>=pages||*loading} onclick={Callback::from({let query=query.clone();move |_|{let mut next=(*query).clone();next.offset+=next.limit;query.set(next);}})}>{"Next"}</button></div></div>
        </section>}
}
