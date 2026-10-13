use crate::{
    api,
    app::{Route, SessionCtx, Shell, signed_out},
    collection_api::CollectionApi,
    confirm::{ConfirmRequest, use_confirm},
    formula_builder::FormulaEditor,
    import::ImportPanel,
};
use vulse_model::{
    ArrayItems, Blueprint, CollectionView, DocumentView, FieldDef, FieldType, Role,
    UpdateCollection, Visibility,
};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::*;

#[derive(Properties, PartialEq)]
pub struct CollectionPageProps {
    pub id: String,
}

#[function_component(CollectionPage)]
pub fn collection_page(props: &CollectionPageProps) -> Html {
    let session = use_context::<SessionCtx>().expect("session");
    let confirm = use_confirm();
    // Saved fields whose removal was confirmed with the Remove button.
    let confirmed_removals = use_state(Vec::<String>::new);
    let collection = use_state(|| None::<CollectionView>);
    let collections = use_state(Vec::<CollectionView>::new);
    let documents = use_state(Vec::<DocumentView>::new);
    let name = use_state(String::new);
    let visibility = use_state(|| Visibility::Private);
    let fields = use_state(Vec::<FieldDef>::new);
    let title_field = use_state(String::new);
    let error = use_state(String::new);
    let status = use_state(String::new);
    let busy = use_state(|| false);
    let tab = use_state(|| "data".to_string());
    let pending = use_state(|| None::<crate::drafts::SchemaDraft>);
    let draft_error = use_state(String::new);
    let draft_version = use_state(|| None::<String>);
    let draft_key = format!(
        "vulse:draft:schema:{}:{}",
        session
            .state
            .user
            .as_ref()
            .map(|u| u.id.as_str())
            .unwrap_or("signedout"),
        props.id
    );
    let edited = Blueprint {
        fields: (*fields).clone(),
        title_field: if title_field.is_empty() {
            None
        } else {
            Some((*title_field).clone())
        },
    };
    let dirty = collection
        .as_ref()
        .is_some_and(|c| c.name != *name || c.visibility != *visibility || c.blueprint != edited);
    let latest = use_mut_ref(|| {
        (
            String::new(),
            Visibility::Private,
            Blueprint {
                fields: Vec::new(),
                title_field: None,
            },
        )
    });
    *latest.borrow_mut() = ((*name).clone(), *visibility, edited.clone());
    // Formulas can read other collections, which may have been created or
    // changed since this page loaded.
    {
        let collections = collections.clone();
        use_effect_with((*tab).clone(), move |tab| {
            if tab == "fields" {
                let collections = collections.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Ok(items) = api::collections().await {
                        collections.set(items);
                    }
                });
            }
        });
    }
    let refresh = use_state(|| 0usize);
    {
        let id = props.id.clone();
        let collection = collection.clone();
        let collections = collections.clone();
        let documents = documents.clone();
        let name = name.clone();
        let visibility = visibility.clone();
        let fields = fields.clone();
        let title_field = title_field.clone();
        let error = error.clone();
        let tab = tab.clone();
        let pending = pending.clone();
        let draft_error = draft_error.clone();
        let key = draft_key.clone();
        let draft_version = draft_version.clone();
        let authed = session.state.user.is_some();
        use_effect_with((id.clone(), authed, *refresh), move |(id, authed, _)| {
            if !*authed {
                return;
            }
            collection.set(None);
            tab.set("data".into());
            error.set(String::new());
            let id = id.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::collection(&id).await {
                    Ok(item) => {
                        draft_version.set(Some(item.updated_at.clone()));
                        match crate::drafts::read(&key) {
                            Ok(value) => pending.set(value),
                            Err(e) => draft_error.set(e),
                        }
                        name.set(item.name.clone());
                        visibility.set(item.visibility);
                        fields.set(item.blueprint.fields.clone());
                        title_field.set(item.blueprint.title_field.clone().unwrap_or_default());
                        collection.set(Some(item));
                    }
                    Err(err) => error.set(err),
                }
                if let Ok(items) = api::collections().await {
                    collections.set(items);
                }
                match api::query_records(&id, &vulse_model::RecordQuery::default()).await {
                    Ok(result) => documents.set(result.items),
                    Err(err) => error.set(err),
                }
            });
        });
    }
    {
        let key = draft_key.clone();
        let draft_error = draft_error.clone();
        let version = (*draft_version).clone();
        use_effect_with(
            (
                edited.clone(),
                (*name).clone(),
                *visibility,
                dirty,
                pending.is_some(),
                version,
                collection.is_some(),
            ),
            move |(blueprint, name, visibility, dirty, pending, version, ready)| {
                if *ready && !*pending {
                    let result = if !*dirty {
                        crate::drafts::clear_readable::<crate::drafts::SchemaDraft>(&key)
                    } else if let Some(version) = version {
                        crate::drafts::write(
                            &key,
                            &crate::drafts::SchemaDraft {
                                name: name.clone(),
                                visibility: *visibility,
                                blueprint: blueprint.clone(),
                                version: version.clone(),
                            },
                        )
                    } else {
                        return;
                    };
                    match result {
                        Ok(()) => draft_error.set(String::new()),
                        Err(e) => draft_error.set(e),
                    }
                }
            },
        );
    }
    let restore_draft = Callback::from({
        let pending = pending.clone();
        let name = name.clone();
        let visibility = visibility.clone();
        let fields = fields.clone();
        let title_field = title_field.clone();
        let draft_version = draft_version.clone();
        move |_| {
            if let Some(d) = (*pending).clone() {
                name.set(d.name);
                visibility.set(d.visibility);
                fields.set(d.blueprint.fields);
                title_field.set(d.blueprint.title_field.unwrap_or_default());
                draft_version.set(Some(d.version));
                pending.set(None);
            }
        }
    });
    let discard_draft = Callback::from({
        let pending = pending.clone();
        let key = draft_key.clone();
        let draft_error = draft_error.clone();
        let confirm = confirm.clone();
        move |_| {
            let pending = pending.clone();
            let key = key.clone();
            let draft_error = draft_error.clone();
            let confirm = confirm.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let request = ConfirmRequest::danger(
                    "Discard this collection draft?",
                    "The unsaved field and setting changes stored on this device are deleted. The saved collection is not affected.",
                    "Discard draft",
                );
                if !confirm.ask(request).await {
                    return;
                }
                match crate::drafts::clear(&key) {
                    Ok(()) => {
                        pending.set(None);
                        draft_error.set(String::new());
                    }
                    Err(e) => draft_error.set(e),
                }
            });
        }
    });
    let use_latest_version = Callback::from({
        let collection = collection.clone();
        let draft_version = draft_version.clone();
        let confirm = confirm.clone();
        move |_| {
            let collection = collection.clone();
            let draft_version = draft_version.clone();
            let confirm = confirm.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let request = ConfirmRequest::new(
                    "Use the latest collection version?",
                    "Your draft fields will be saved against the latest collection. Review all fields first; existing records are validated before any change.",
                    "Use latest version",
                );
                if confirm.ask(request).await {
                    draft_version.set(collection.as_ref().map(|c| c.updated_at.clone()));
                }
            });
        }
    });
    let on_save = {
        let id = props.id.clone();
        let name = name.clone();
        let visibility = visibility.clone();
        let fields = fields.clone();
        let title_field = title_field.clone();
        let collection = collection.clone();
        let collections = collections.clone();
        let error = error.clone();
        let status = status.clone();
        let busy = busy.clone();
        let documents = documents.clone();
        let draft_version = draft_version.clone();
        let key = draft_key.clone();
        let latest = latest.clone();
        let draft_error = draft_error.clone();
        let confirm = confirm.clone();
        let confirmed_removals = confirmed_removals.clone();
        Callback::from(move |_| {
            if *busy {
                return;
            }
            // Saved fields missing from the draft lose their stored values.
            let removed: Vec<String> = collection
                .as_ref()
                .map(|c| {
                    c.blueprint
                        .fields
                        .iter()
                        .filter(|f| f.kind != FieldType::Calculated)
                        .filter(|f| !fields.iter().any(|next| next.name == f.name))
                        .filter(|f| {
                            !fields
                                .iter()
                                .any(|next| next.renamed_from.as_deref() == Some(f.name.as_str()))
                        })
                        .map(|f| f.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            // Saved fields (followed through renames) whose type changes.
            let retyped: Vec<String> = collection
                .as_ref()
                .map(|c| {
                    fields
                        .iter()
                        .filter(|f| f.kind != FieldType::Calculated)
                        .filter_map(|f| {
                            let origin = f.renamed_from.as_deref().unwrap_or(&f.name);
                            let old = c.blueprint.fields.iter().find(|o| o.name == origin)?;
                            (old.kind != FieldType::Calculated
                                && (old.kind != f.kind || old.items != f.items))
                                .then(|| {
                                    format!(
                                        "{} ({} → {})",
                                        f.display_label(),
                                        type_label(old.kind),
                                        type_label(f.kind)
                                    )
                                })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let unconfirmed: Vec<String> = removed
                .iter()
                .filter(|name| !confirmed_removals.contains(name))
                .cloned()
                .collect();
            busy.set(true);
            status.set(String::new());
            error.set(String::new());
            let body = UpdateCollection {
                expected_updated_at: (*draft_version).clone(),
                name: Some((*name).clone()),
                visibility: Some(*visibility),
                blueprint: Some(Blueprint {
                    title_field: if title_field.is_empty() {
                        None
                    } else {
                        Some((*title_field).clone())
                    },
                    fields: (*fields).clone(),
                }),
                remove_field_data: !removed.is_empty(),
                convert_field_values: !retyped.is_empty(),
            };
            let name = name.clone();
            let fields = fields.clone();
            let visibility = visibility.clone();
            let title_field = title_field.clone();
            let submitted = (
                (*name).clone(),
                *visibility,
                body.blueprint.clone().unwrap(),
            );
            let (draft_version, key, latest, draft_error) = (
                draft_version.clone(),
                key.clone(),
                latest.clone(),
                draft_error.clone(),
            );
            let id = id.clone();
            let collection = collection.clone();
            let collections = collections.clone();
            let error = error.clone();
            let status = status.clone();
            let busy = busy.clone();
            let documents = documents.clone();
            let confirm = confirm.clone();
            let confirmed_removals = confirmed_removals.clone();
            wasm_bindgen_futures::spawn_local(async move {
                // Renamed fields count as removed: their values do not move.
                if !unconfirmed.is_empty() {
                    let request = ConfirmRequest::danger(
                        "Delete data in removed fields?",
                        format!(
                            "These saved fields are no longer in the collection: {}. Saving deletes their values from every record. Each record's previous version stays in its history.",
                            unconfirmed.join(", ")
                        ),
                        "Delete field data and save",
                    );
                    if !confirm.ask(request).await {
                        busy.set(false);
                        return;
                    }
                }
                if !retyped.is_empty() {
                    let request = ConfirmRequest::new(
                        "Convert existing values?",
                        format!(
                            "Changing the type of {} converts every stored value, for example the text \"12\" becomes the number 12, and blank text becomes empty. If any value cannot be converted, nothing is saved and you'll see which records to fix. Each record's previous version stays in its history.",
                            retyped.join(", ")
                        ),
                        "Convert and save",
                    );
                    if !confirm.ask(request).await {
                        busy.set(false);
                        return;
                    }
                }
                match api::update_collection(&id, &body).await {
                    Ok(item) => {
                        confirmed_removals.set(Vec::new());
                        draft_version.set(Some(item.updated_at.clone()));
                        if *latest.borrow() == submitted {
                            name.set(item.name.clone());
                            visibility.set(item.visibility);
                            fields.set(item.blueprint.fields.clone());
                            title_field.set(item.blueprint.title_field.clone().unwrap_or_default());
                            if let Err(e) = crate::drafts::clear(&key) {
                                draft_error.set(e);
                            }
                        }
                        collection.set(Some(item));
                        status.set("Changes saved".into());
                        if let Ok(items) = api::collections().await {
                            collections.set(items);
                        }
                        match api::query_records(&id, &vulse_model::RecordQuery::default()).await {
                            Ok(result) => documents.set(result.items),
                            Err(e) => error.set(e),
                        }
                    }
                    Err(err) => error.set(err),
                }
                busy.set(false);
            });
        })
    };
    let navigator = use_navigator();
    let on_delete = {
        let id = props.id.clone();
        let error = error.clone();
        let busy = busy.clone();
        let confirm = confirm.clone();
        let collection_name = collection
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        Callback::from(move |_| {
            if *busy {
                return;
            }
            let id = id.clone();
            let error = error.clone();
            let navigator = navigator.clone();
            let busy = busy.clone();
            let confirm = confirm.clone();
            let request = ConfirmRequest::danger(
                format!("Delete the collection “{collection_name}”?"),
                "All its records, fields, saved views and attached files are permanently deleted. This cannot be undone.",
                "Delete collection",
            );
            wasm_bindgen_futures::spawn_local(async move {
                if !confirm.ask(request).await {
                    return;
                }
                busy.set(true);
                match api::delete_collection(&id).await {
                    Ok(()) => {
                        if let Some(n) = navigator {
                            n.push(&Route::Home);
                        }
                    }
                    Err(e) => error.set(e),
                }
                busy.set(false);
            });
        })
    };
    if let Some(redirect) = signed_out(&session) {
        return redirect;
    }
    html! {
        <Shell>
            <p class="breadcrumb"><Link<Route> to={Route::Home}>{"Workspace"}</Link<Route>><span>{" / Collections"}</span></p>
            if let Some(item)=collection.as_ref() {
                <div class="page-heading"><div><p class="eyebrow">{"COLLECTION"}</p><h1>{item.name.clone()}</h1><p class="muted">{format!("{} fields",item.blueprint.fields.len())}</p></div>
                    <div class="row"><span class={if item.visibility==Visibility::Public {"badge public"}else{"badge"}}>{if item.visibility==Visibility::Public{"Public"}else{"Private"}}</span>
                    if session.can_edit() {<Link<Route> classes="button" to={Route::Document{id:props.id.clone(),doc_id:"new".into()}}>{"+ Add record"}</Link<Route>>}</div>
                </div>
                <nav class="tabs" aria-label="Collection views">{for [("data","Records",Role::Viewer),("fields","Fields",Role::Admin),("import","Import data",Role::Editor),("api","API",Role::Viewer),("trash","Trash",Role::Viewer),("settings","Settings",Role::Admin)].into_iter().filter(|(_,_,role)|session.role()>=*role).map(|(value,label,_)|{
                    let tab=tab.clone();html!{<button type="button" class={if *tab==value{"active"}else{""}} aria-current={if *tab==value{Some("page")}else{None}} onclick={Callback::from(move |_|tab.set(value.into()))}>{label}</button>}
                })}</nav>
                if !error.is_empty(){<p class="error" role="alert">{(*error).clone()}</p>}
                if !status.is_empty(){<p class="success" role="status">{(*status).clone()}</p>}
                if pending.is_some(){<section class="card draft-banner"><strong>{"Unsaved collection draft found on this device"}</strong><p>{"Restore your fields and settings to review them. An older draft will be checked for conflicts before saving."}</p><div class="row"><button class="secondary" onclick={restore_draft}>{"Restore collection draft"}</button><button class="danger" onclick={discard_draft.clone()}>{"Discard collection draft"}</button></div></section>}
                if !draft_error.is_empty(){<p class="error" role="alert">{(*draft_error).clone()}</p><button class="secondary" onclick={discard_draft}>{"Discard unreadable collection draft"}</button>}
                if draft_version.as_ref()!=Some(&item.updated_at){<section class="card draft-banner"><p>{"This draft was based on an older collection version. Review the fields against the current collection before saving."}</p><details><summary>{"Current collection fields"}</summary><pre>{serde_json::to_string_pretty(&item.blueprint).unwrap_or_default()}</pre></details><button class="secondary" onclick={use_latest_version}>{"Use latest collection version"}</button></section>}
                if dirty {<p role="status" class="muted">{if draft_error.is_empty(){"Unsaved fields and settings · draft saved on this device"}else{"Unsaved fields and settings · draft storage unavailable"}}</p>}
                if *tab=="trash" {<crate::history::HistoryPanel collection={item.clone()}/>}
                if *tab=="data" {<crate::records::RecordsPanel key={item.id.clone()} collection={item.clone()} collections={(*collections).clone()}/>}
                if *tab=="fields" {<fieldset class="draft-fields" disabled={*busy||pending.is_some()}>
                    <div class="section-heading"><div><h2>{"Shape your collection"}</h2><p class="muted">{"Choose the fields each record contains. Calculated fields update automatically."}</p></div><button disabled={*busy} onclick={on_save.clone()}>{if *busy{"Saving…"}else{"Save fields"}}</button></div>
                    <section class="card"><h2>{"Record titles"}</h2><p class="muted">{"Choose the field shown in record links and reference pickers. API field names stay unchanged."}</p><label>{"Title field"}<select aria-label="Title field" onchange={Callback::from({let title_field=title_field.clone();move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();title_field.set(select.value());}})}><option value="" selected={title_field.is_empty()}>{"Automatic · first text field"}</option>{for fields.iter().filter(|f|matches!(f.kind,FieldType::String|FieldType::Number|FieldType::Boolean|FieldType::Date|FieldType::Calculated)).map(|f|html!{<option value={f.name.clone()} selected={*title_field==f.name}>{f.display_label().to_string()}</option>})}</select></label></section><BlueprintEditor records={(*documents).clone()} fields={(*fields).clone()} collections={(*collections).clone()} current={item.name.clone()} saved_fields={item.blueprint.fields.iter().map(|f|f.name.clone()).collect::<Vec<_>>()} on_renamed={Callback::from({let title_field=title_field.clone();move |(old,new):(String,String)|{if *title_field==old{title_field.set(new);}}})} on_removed={Callback::from({let confirmed=confirmed_removals.clone();move |name:String|{let mut next=(*confirmed).clone();next.push(name);confirmed.set(next);}})} on_change={Callback::from({let fields=fields.clone();move |next|fields.set(next)})}/>
                </fieldset>}
                if *tab=="import" {<ImportPanel key={props.id.clone()} collection={item.clone()} on_import={Callback::from({let refresh=refresh.clone();move |_|refresh.set(*refresh+1)})}/>}
                if *tab=="api" {<CollectionApi collection={item.clone()}/>}
                if *tab=="settings" {<fieldset class="draft-fields" disabled={*busy||pending.is_some()}>
                    <section class="card settings-card"><h2>{"Collection settings"}</h2><label>{"Collection name"}<input value={(*name).clone()} oninput={Callback::from({let name=name.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();name.set(input.value().to_ascii_lowercase());}})}/></label><label>{"Who can read your data?"}<select onchange={Callback::from({let visibility=visibility.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();visibility.set(if input.value()=="public"{Visibility::Public}else{Visibility::Private});}})}><option value="private" selected={*visibility==Visibility::Private}>{"Private — workspace members and API keys"}</option><option value="public" selected={*visibility==Visibility::Public}>{"Public — anyone with the API address"}</option></select></label><button disabled={*busy} onclick={on_save}>{if *busy{"Saving…"}else{"Save changes"}}</button></section>
                    </fieldset><section class="card danger-zone"><h2>{"Delete collection"}</h2><p class="muted">{"Permanently remove all records, fields and attached files."}</p><button class="danger" disabled={*busy} onclick={on_delete}>{"Delete collection…"}</button></section>
                }
            }else if error.is_empty(){<p role="status" class="muted">{"Loading collection…"}</p>}else{<p class="error" role="alert">{(*error).clone()}</p>}
        </Shell>
    }
}

#[derive(Properties, PartialEq)]
struct BlueprintEditorProps {
    records: Vec<DocumentView>,
    fields: Vec<FieldDef>,
    collections: Vec<CollectionView>,
    current: String,
    /// Field names in the saved collection.
    saved_fields: Vec<String>,
    /// A saved field was removed after confirmation.
    on_removed: Callback<String>,
    /// A field name was committed: (previous name, new name).
    on_renamed: Callback<(String, String)>,
    on_change: Callback<Vec<FieldDef>>,
}

#[function_component(BlueprintEditor)]
fn blueprint_editor(props: &BlueprintEditorProps) -> Html {
    let add = {
        let fields = props.fields.clone();
        let on_change = props.on_change.clone();
        Callback::from(move |_| {
            let mut next = fields.clone();
            let mut number = next.len() + 1;
            while next.iter().any(|f| f.name == format!("field_{number}")) {
                number += 1;
            }
            next.push(FieldDef {
                label: None,
                description: None,
                example: None,
                name: format!("field_{number}"),
                required: false,
                kind: FieldType::String,
                collection: None,
                items: None,
                formula: None,
                renamed_from: None,
            });
            on_change.emit(next);
        })
    };
    html! {
        <section class="card">
            <h2>{"Collection fields"}</h2>
            if props.fields.is_empty() {
                <p class="muted">{"Add your first field to start shaping your records."}</p>
            }
            { for props.fields.iter().enumerate().map(|(index, field)| {
                html! {
                    <FieldRow
                        key={index}
                        records={props.records.clone()}
                        index={index}
                        field={field.clone()}
                        total={props.fields.len()}
                        collections={props.collections.clone()}
                        current={props.current.clone()}
                        fields={props.fields.clone()}
                        origin={field.renamed_from.clone().or_else(|| {
                            // A saved name claimed by a renamed row belongs to that row.
                            (props.saved_fields.contains(&field.name)
                                && !props.fields.iter().any(|f| f.renamed_from.as_deref() == Some(field.name.as_str())))
                            .then(|| field.name.clone())
                        })}
                        on_removed={props.on_removed.clone()}
                        on_renamed={props.on_renamed.clone()}
                        on_change={props.on_change.clone()}
                    />
                }
            }) }
            <button type="button" onclick={add}>{"Add field"}</button>
        </section>
    }
}

#[derive(Properties, PartialEq)]
struct FieldRowProps {
    records: Vec<DocumentView>,
    index: usize,
    total: usize,
    field: FieldDef,
    collections: Vec<CollectionView>,
    current: String,
    fields: Vec<FieldDef>,
    /// The saved field this row edits, under its saved name, if any.
    origin: Option<String>,
    on_removed: Callback<String>,
    on_renamed: Callback<(String, String)>,
    on_change: Callback<Vec<FieldDef>>,
}

#[function_component(FieldRow)]
fn field_row(props: &FieldRowProps) -> Html {
    let confirm = use_confirm();
    let patch = {
        let fields = props.fields.clone();
        let index = props.index;
        let on_change = props.on_change.clone();
        Callback::from(move |next: FieldDef| {
            let mut fields = fields.clone();
            if index < fields.len() {
                fields[index] = next;
                on_change.emit(fields);
            }
        })
    };
    let move_by = {
        let fields = props.fields.clone();
        let index = props.index;
        let on_change = props.on_change.clone();
        Callback::from(move |delta: isize| {
            let mut fields = fields.clone();
            let target = index as isize + delta;
            if target >= 0 && (target as usize) < fields.len() {
                fields.swap(index, target as usize);
                on_change.emit(fields);
            }
        })
    };
    let remove = {
        let fields = props.fields.clone();
        let index = props.index;
        let on_change = props.on_change.clone();
        let on_removed = props.on_removed.clone();
        // Calculated fields store nothing, so only stored saved fields ask.
        let saved = props
            .origin
            .clone()
            .filter(|_| props.field.kind != FieldType::Calculated);
        let label = props.field.display_label().to_string();
        let has_values = saved.as_ref().is_some_and(|origin| {
            props
                .records
                .iter()
                .any(|r| r.body.get(origin).is_some_and(|v| !v.is_null()))
        });
        Callback::from(move |_| {
            let mut fields = fields.clone();
            if index >= fields.len() {
                return;
            }
            fields.remove(index);
            // A field that was never saved holds no data.
            let Some(name) = saved.clone() else {
                on_change.emit(fields);
                return;
            };
            let (confirm, on_change, on_removed) =
                (confirm.clone(), on_change.clone(), on_removed.clone());
            let message = format!(
                "“{label}” is removed from the form now. When you save the fields, {} Each record's previous version stays in its history.",
                if has_values {
                    "its values are deleted from every record that has one."
                } else {
                    "any values stored in it are deleted from every record."
                }
            );
            let request = ConfirmRequest::danger(
                format!("Remove the field “{label}”?"),
                message,
                "Remove field",
            );
            wasm_bindgen_futures::spawn_local(async move {
                if confirm.ask(request).await {
                    on_change.emit(fields);
                    on_removed.emit(name);
                }
            });
        })
    };
    // Formulas follow a committed name change. Done on commit, not per
    // keystroke, so intermediate names never capture other fields' references.
    let focused_name = use_mut_ref(|| None::<String>);
    let commit_name = {
        let fields = props.fields.clone();
        let index = props.index;
        let on_change = props.on_change.clone();
        let on_renamed = props.on_renamed.clone();
        let focused = focused_name.clone();
        Callback::from(move |_: Event| {
            let Some(old) = focused.borrow_mut().take() else {
                return;
            };
            let Some(new) = fields.get(index).map(|f| f.name.clone()) else {
                return;
            };
            *focused.borrow_mut() = Some(new.clone());
            let others = |name: &str| {
                fields
                    .iter()
                    .enumerate()
                    .any(|(i, f)| i != index && f.name == name)
            };
            if old == new || new.is_empty() || others(&old) || others(&new) {
                return;
            }
            let renames = std::collections::BTreeMap::from([(old.clone(), new.clone())]);
            let mut next = fields.clone();
            for field in &mut next {
                if let Some(formula) = &field.formula {
                    field.formula = Some(vulse_model::rename_formula_fields(formula, &renames));
                }
            }
            if next != fields {
                on_change.emit(next);
            }
            on_renamed.emit((old, new));
        })
    };
    let field = props.field.clone();
    html! {
        <div class="field-row">
            <label>{"Name"}
                <input aria-label="Name" value={field.name.clone()} onfocus={Callback::from({
                    let focused = focused_name.clone();
                    let name = field.name.clone();
                    move |_| *focused.borrow_mut() = Some(name.clone())
                })} oninput={Callback::from({
                    let patch = patch.clone();
                    let field = field.clone();
                    let origin = props.origin.clone();
                    move |event: InputEvent| {
                        let element: HtmlInputElement = event.target_unchecked_into();
                        let mut next = field.clone();
                        next.name = element.value().to_ascii_lowercase();
                        // Renaming a saved field moves its data on save.
                        next.renamed_from = origin.clone().filter(|o| *o != next.name);
                        patch.emit(next);
                    }
                })} onchange={commit_name} />
                if let Some(from) = &field.renamed_from {
                    <span class="field-help">{format!("Renamed from “{from}”. Values, formulas and saved views follow the new name when you save.")}</span>
                }
            </label>
            <label>{"Type"}
                <select aria-label="Type" onchange={Callback::from({
                    let patch = patch.clone();
                    let field = field.clone();
                    let props_current = props.current.clone();
                    move |event: Event| {
                        let element: HtmlSelectElement = event.target_unchecked_into();
                        let mut next = field.clone();
                        next.kind = parse_type(&element.value());
                        next.collection = if next.kind == FieldType::Reference || (next.kind == FieldType::Array && next.items == Some(ArrayItems::Reference)) {
                            Some(props_current.clone())
                        } else {
                            None
                        };
                        next.items = if next.kind == FieldType::Array {
                            Some(next.items.unwrap_or(ArrayItems::String))
                        } else {
                            None
                        };
                        next.formula = if next.kind == FieldType::Calculated {Some(String::new())} else {None};
                        if next.kind == FieldType::Calculated {next.required=false;}
                        patch.emit(next);
                    }
                })}>
                    { for field_types().into_iter().map(|(value, label)| html! {
                        <option value={value} selected={type_value(field.kind) == value}>{label}</option>
                    }) }
                </select>
            </label>
            if field.kind != FieldType::Calculated {
            <label class="check">
                <input type="checkbox" checked={field.required} disabled={field.kind == FieldType::Calculated} onchange={Callback::from({
                    let patch = patch.clone();
                    let field = field.clone();
                    move |event: Event| {
                        let element: HtmlInputElement = event.target_unchecked_into();
                        let mut next = field.clone();
                        next.required = element.checked();
                        patch.emit(next);
                    }
                })} />
                {"Required"}
            </label>
            }
            <div class="field-metadata">{for [("Display label", "label", field.label.clone(), 200), ("Description", "description", field.description.clone(), 1000), ("Example", "example", field.example.clone(), 500)].into_iter().map(|(caption, key, value, limit)| {
                let patch=patch.clone(); let field=field.clone(); html!{<label>{caption}<input maxlength={limit.to_string()} value={value.unwrap_or_default()} oninput={Callback::from(move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();let mut next=field.clone();let value=if input.value().is_empty(){None}else{Some(input.value())};match key{"label"=>next.label=value,"description"=>next.description=value,_=>next.example=value};patch.emit(next);})}/></label>}
            })}<p class="muted">{"The name is the stable API key. Display labels and help text can change without changing stored data or formulas."}</p></div>
            if field.kind == FieldType::Reference
                || (field.kind == FieldType::Array && field.items == Some(ArrayItems::Reference))
            {
                <label>{"Collection"}
                    <select aria-label="Collection" onchange={Callback::from({
                        let patch = patch.clone();
                        let field = field.clone();
                        move |event: Event| {
                            let element: HtmlSelectElement = event.target_unchecked_into();
                            let mut next = field.clone();
                            next.collection = Some(element.value());
                            patch.emit(next);
                        }
                    })}>
                        <option value={props.current.clone()} selected={field.collection.as_deref() == Some(props.current.as_str())}>{"This collection"}</option>
                        { for props.collections.iter().filter(|item| item.name != props.current).map(|item| {
                            let selected = field.collection.as_deref() == Some(item.name.as_str());
                            html! { <option value={item.name.clone()} {selected}>{item.name.clone()}</option> }
                        }) }
                    </select>
                </label>
            }
            if field.kind == FieldType::Array {
                <label>{"Items"}
                    <select aria-label="Items" onchange={Callback::from({
                        let patch = patch.clone();
                        let field = field.clone();
                        let current = props.current.clone();
                        move |event: Event| {
                            let element: HtmlSelectElement = event.target_unchecked_into();
                            let mut next = field.clone();
                            next.items = Some(parse_items(&element.value()));
                            next.collection = if next.items == Some(ArrayItems::Reference) { Some(current.clone()) } else {None};
                            patch.emit(next);
                        }
                    })}>
                        <option value="string" selected={field.items == Some(ArrayItems::String)}>{"String"}</option>
                        <option value="number" selected={field.items == Some(ArrayItems::Number)}>{"Number"}</option>
                        <option value="asset" selected={field.items == Some(ArrayItems::Asset)}>{"Asset"}</option>
                        <option value="reference" selected={field.items == Some(ArrayItems::Reference)}>{"Reference"}</option>
                    </select>
                </label>
            }
            if field.kind == FieldType::Calculated {
                <FormulaEditor records={props.records.clone()} field={field.clone()} fields={props.fields.clone()} collections={props.collections.clone()} current={props.current.clone()} on_change={patch.clone()}/>
            }
            <div class="row field-actions">
                <button class="secondary" type="button" disabled={props.index == 0} onclick={Callback::from({
                    let move_by = move_by.clone();
                    move |_| move_by.emit(-1)
                })}>{"Up"}</button>
                <button class="secondary" type="button" disabled={props.index + 1 == props.total} onclick={Callback::from({
                    let move_by = move_by.clone();
                    move |_| move_by.emit(1)
                })}>{"Down"}</button>
                <button type="button" class="danger" onclick={remove}>{"Remove"}</button>
            </div>
        </div>
    }
}

fn field_types() -> Vec<(&'static str, &'static str)> {
    vec![
        ("calculated", "ƒx Calculated"),
        ("string", "Text"),
        ("number", "Number"),
        ("boolean", "Boolean"),
        ("date", "Date"),
        ("json", "JSON"),
        ("portable_text", "Portable Text"),
        ("asset", "Asset"),
        ("reference", "Reference"),
        ("array", "Array"),
    ]
}

fn type_value(kind: FieldType) -> &'static str {
    match kind {
        FieldType::Calculated => "calculated",
        FieldType::String => "string",
        FieldType::Number => "number",
        FieldType::Boolean => "boolean",
        FieldType::Date => "date",
        FieldType::Json => "json",
        FieldType::PortableText => "portable_text",
        FieldType::Asset => "asset",
        FieldType::Reference => "reference",
        FieldType::Array => "array",
    }
}

fn parse_type(value: &str) -> FieldType {
    match value {
        "calculated" => FieldType::Calculated,
        "number" => FieldType::Number,
        "boolean" => FieldType::Boolean,
        "date" => FieldType::Date,
        "json" => FieldType::Json,
        "portable_text" => FieldType::PortableText,
        "asset" => FieldType::Asset,
        "reference" => FieldType::Reference,
        "array" => FieldType::Array,
        _ => FieldType::String,
    }
}

fn parse_items(value: &str) -> ArrayItems {
    match value {
        "number" => ArrayItems::Number,
        "asset" => ArrayItems::Asset,
        "reference" => ArrayItems::Reference,
        _ => ArrayItems::String,
    }
}

/// Readable field type for conversion prompts.
fn type_label(kind: FieldType) -> &'static str {
    field_types()
        .iter()
        .find(|(value, _)| *value == type_value(kind))
        .map(|(_, label)| *label)
        .unwrap_or("value")
}
