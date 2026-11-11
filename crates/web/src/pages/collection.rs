use crate::{
    api,
    app::{Route, SessionCtx, Shell, signed_out},
    collection_api::CollectionApi,
    confirm::{ConfirmRequest, use_confirm},
    field_import::FieldsFromFile,
    formula_builder::FormulaEditor,
    import::ImportPanel,
};
use std::collections::BTreeSet;
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
    // The editor rows, with the saved field each one edits (see `mark_saved`).
    // Drafts keep these markers.
    let edited = Blueprint {
        fields: (*fields).clone(),
        title_field: if title_field.is_empty() {
            None
        } else {
            Some((*title_field).clone())
        },
    };
    // The blueprint as the server reads it.
    let unmarked = Blueprint {
        fields: strip_self_marks(&fields),
        title_field: edited.title_field.clone(),
    };
    let dirty = collection
        .as_ref()
        .is_some_and(|c| c.name != *name || c.visibility != *visibility || c.blueprint != unmarked);
    // Collapsed field rows (see `BlueprintEditor`), kept here so each row
    // stays open or collapsed across a save.
    let collapsed = use_state(|| None::<BTreeSet<String>>);
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
    *latest.borrow_mut() = ((*name).clone(), *visibility, unmarked);
    // Formulas can read other collections, which may have been created or
    // changed since this page loaded.
    {
        let id = props.id.clone();
        let collections = collections.clone();
        let documents = documents.clone();
        use_effect_with((*tab).clone(), move |tab| {
            if tab == "fields" {
                let id = id.clone();
                let collections = collections.clone();
                let documents = documents.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Ok(items) = api::collections().await {
                        collections.set(items);
                    }
                    let sample_query = vulse_model::RecordQuery {
                        limit: 100,
                        ..vulse_model::RecordQuery::default()
                    };
                    if let Ok(result) = api::query_records(&id, &sample_query).await {
                        documents.set(result.items);
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
        let collapsed = collapsed.clone();
        let confirmed_removals = confirmed_removals.clone();
        let authed = session.state.user.is_some();
        use_effect_with((id.clone(), authed, *refresh), move |(id, authed, _)| {
            if !*authed {
                return;
            }
            collection.set(None);
            collapsed.set(None);
            confirmed_removals.set(Vec::new());
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
                        fields.set(mark_saved(&item.blueprint.fields));
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
        let collection = collection.clone();
        let confirmed_removals = confirmed_removals.clone();
        move |_| {
            if let Some(d) = (*pending).clone() {
                let saved: Vec<String> = collection
                    .as_ref()
                    .map(|c| c.blueprint.fields.iter().map(|f| f.name.clone()).collect())
                    .unwrap_or_default();
                confirmed_removals.set(Vec::new());
                name.set(d.name);
                visibility.set(d.visibility);
                fields.set(adopt_saved_rows(d.blueprint.fields, &saved));
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
        let confirmed_removals = confirmed_removals.clone();
        move |_| {
            let pending = pending.clone();
            let key = key.clone();
            let draft_error = draft_error.clone();
            let confirm = confirm.clone();
            let confirmed_removals = confirmed_removals.clone();
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
                        confirmed_removals.set(Vec::new());
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
        let collapsed = collapsed.clone();
        Callback::from(move |_| {
            if *busy {
                return;
            }
            // Saved fields missing from the draft, or replaced by a rename
            // onto their name, lose their stored values.
            let removed: Vec<String> = collection
                .as_ref()
                .map(|c| removed_fields(&c.blueprint.fields, &fields))
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
                    fields: strip_self_marks(&fields),
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
            // The rows and which are collapsed, which cannot change while the
            // save is pending: the editor is disabled.
            let (rows_before, collapsed) = ((*fields).clone(), collapsed.clone());
            let saved_before: Vec<String> = collection
                .as_ref()
                .map(|c| c.blueprint.fields.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();
            let was_collapsed = (*collapsed)
                .clone()
                .unwrap_or_else(|| default_collapsed(&saved_before));
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
                            let rows = mark_saved(&item.blueprint.fields);
                            let saved_after: Vec<String> =
                                rows.iter().map(|f| f.name.clone()).collect();
                            collapsed.set(Some(carry_collapsed(
                                &was_collapsed,
                                &rows_before,
                                &saved_before,
                                &rows,
                                &saved_after,
                            )));
                            fields.set(rows);
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
                    <section class="card"><h2>{"Record titles"}</h2><p class="muted">{"Choose the field shown in record links and reference pickers. API field names stay unchanged."}</p><label>{"Title field"}<select aria-label="Title field" onchange={Callback::from({let title_field=title_field.clone();move |e:Event|{let select:HtmlSelectElement=e.target_unchecked_into();title_field.set(select.value());}})}><option value="" selected={title_field.is_empty()}>{"Automatic · first text field"}</option>{for fields.iter().filter(|f|matches!(f.kind,FieldType::String|FieldType::Number|FieldType::Boolean|FieldType::Date|FieldType::Calculated)).map(|f|html!{<option value={f.name.clone()} selected={*title_field==f.name}>{f.display_label().to_string()}</option>})}</select></label></section><BlueprintEditor collection_id={item.id.clone()} records={(*documents).clone()} fields={(*fields).clone()} collections={(*collections).clone()} current={item.name.clone()} saved_fields={item.blueprint.fields.iter().map(|f|f.name.clone()).collect::<Vec<_>>()} saved_options={item.blueprint.fields.iter().filter_map(|f|Some((f.name.clone(),f.options.clone()?))).collect::<std::collections::BTreeMap<_,_>>()} on_renamed={Callback::from({let title_field=title_field.clone();move |(old,new):(String,String)|{if *title_field==old{title_field.set(new);}}})} on_removed={Callback::from({let confirmed=confirmed_removals.clone();move |name:String|{let mut next=(*confirmed).clone();next.push(name);confirmed.set(next);}})} on_change={Callback::from({let fields=fields.clone();move |next|fields.set(next)})} collapsed={(*collapsed).clone()} on_collapse={Callback::from({let collapsed=collapsed.clone();move |next|collapsed.set(Some(next))})}/>
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
    collection_id: String,
    records: Vec<DocumentView>,
    fields: Vec<FieldDef>,
    collections: Vec<CollectionView>,
    current: String,
    /// Field names in the saved collection.
    saved_fields: Vec<String>,
    /// Allowed values of saved fields, by saved field name.
    saved_options: std::collections::BTreeMap<String, Vec<String>>,
    /// A saved field was removed after confirmation.
    on_removed: Callback<String>,
    /// A field name was committed: (previous name, new name).
    on_renamed: Callback<(String, String)>,
    on_change: Callback<Vec<FieldDef>>,
    /// Collapsed rows by `collapse_key`, once any row was toggled.
    collapsed: Option<BTreeSet<String>>,
    on_collapse: Callback<BTreeSet<String>>,
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
                number_format: None,
                options: None,
                renamed_options: None,
            });
            on_change.emit(next);
        })
    };
    let add_many = {
        let fields = props.fields.clone();
        let on_change = props.on_change.clone();
        Callback::from(move |added: Vec<FieldDef>| {
            let mut next = fields.clone();
            next.extend(added);
            on_change.emit(next);
        })
    };
    // Collapsed rows by `collapse_key`; saved fields start collapsed in long
    // lists. Typing a name never changes the key of a saved row, and unsaved
    // rows have keys of their own, so a row never collapses while edited.
    let collapsed_keys = props
        .collapsed
        .clone()
        .unwrap_or_else(|| default_collapsed(&props.saved_fields));
    let keys: Vec<String> = props
        .fields
        .iter()
        .map(|f| collapse_key(f, &props.saved_fields))
        .collect();
    let set_all = |collapse: bool| {
        let on_collapse = props.on_collapse.clone();
        let keys: BTreeSet<String> = keys.iter().cloned().collect();
        Callback::from(move |_| {
            on_collapse.emit(if collapse {
                keys.clone()
            } else {
                Default::default()
            })
        })
    };
    let all_open = keys.iter().all(|k| !collapsed_keys.contains(k));
    let all_closed = keys.iter().all(|k| collapsed_keys.contains(k));
    html! {
        <section class="card">
            <div class="fields-heading">
                <h2>{"Collection fields"}</h2>
                if props.fields.len() > 1 {
                    <div class="row">
                        <button type="button" class="secondary" disabled={all_open} onclick={set_all(false)}>{"Expand all"}</button>
                        <button type="button" class="secondary" disabled={all_closed} onclick={set_all(true)}>{"Collapse all"}</button>
                    </div>
                }
            </div>
            if props.fields.is_empty() {
                <p class="muted">{"Add your first field to start shaping your records."}</p>
            }
            { for props.fields.iter().enumerate().map(|(index, field)| {
                let origin = row_origin(field, &props.saved_fields);
                let saved_options = origin
                    .as_ref()
                    .and_then(|name| props.saved_options.get(name))
                    .cloned()
                    .unwrap_or_default();
                let key = keys[index].clone();
                let open = !collapsed_keys.contains(&key);
                let on_toggle = {
                    let on_collapse = props.on_collapse.clone();
                    let keys = collapsed_keys.clone();
                    Callback::from(move |_| {
                        let mut next = keys.clone();
                        if !next.remove(&key) {
                            next.insert(key.clone());
                        }
                        on_collapse.emit(next);
                    })
                };
                html! {
                    <FieldRow
                        {open}
                        {on_toggle}
                        key={index}
                        records={props.records.clone()}
                        index={index}
                        field={field.clone()}
                        total={props.fields.len()}
                        collections={props.collections.clone()}
                        current={props.current.clone()}
                        fields={props.fields.clone()}
                        {origin}
                        {saved_options}
                        on_removed={props.on_removed.clone()}
                        on_renamed={props.on_renamed.clone()}
                        on_change={props.on_change.clone()}
                    />
                }
            }) }
            <div class="row"><button type="button" onclick={add}>{"Add field"}</button><FieldsFromFile collection_id={props.collection_id.clone()} fields={props.fields.clone()} on_add={add_many}/></div>
        </section>
    }
}

// Fields have no id; the server knows a field by its name, and `renamed_from`
// in an update moves a saved field's values to a new name. In the editor every
// row edited from a saved field keeps that field's saved name in
// `renamed_from`, even while the name is unchanged (a self-marker), and rows
// added in the editor have none. A row's identity therefore never follows the
// name it currently holds, so typing a new field's name through a saved name
// cannot turn it into that field. Self-markers are removed before comparing
// with or sending to the server.

/// Saved fields as editor rows, each marked as editing itself.
fn mark_saved(fields: &[FieldDef]) -> Vec<FieldDef> {
    fields
        .iter()
        .cloned()
        .map(|mut field| {
            field.renamed_from = Some(field.name.clone());
            field
        })
        .collect()
}

/// Editor rows as the server reads them: self-markers removed.
fn strip_self_marks(fields: &[FieldDef]) -> Vec<FieldDef> {
    fields
        .iter()
        .cloned()
        .map(|mut field| {
            if field.renamed_from.as_deref() == Some(field.name.as_str()) {
                field.renamed_from = None;
            }
            field
        })
        .collect()
}

/// Rows of a restored draft. Drafts from before self-markers mark only
/// renamed rows, so an unmarked row named like a saved field that no other
/// row edits is taken to be that field. Only done once, on restore; while
/// editing, a row's name never decides what it is.
fn adopt_saved_rows(fields: Vec<FieldDef>, saved: &[String]) -> Vec<FieldDef> {
    let mut claimed: std::collections::BTreeSet<String> = fields
        .iter()
        .filter_map(|f| f.renamed_from.clone())
        .collect();
    fields
        .into_iter()
        .map(|mut field| {
            if field.renamed_from.is_none()
                && saved.contains(&field.name)
                && claimed.insert(field.name.clone())
            {
                field.renamed_from = Some(field.name.clone());
            }
            field
        })
        .collect()
}

/// The saved field a row edits, under its saved name. A marker naming a field
/// the collection no longer has (a draft saved against an older version)
/// edits nothing.
fn row_origin(field: &FieldDef, saved: &[String]) -> Option<String> {
    field
        .renamed_from
        .clone()
        .filter(|origin| saved.contains(origin))
}

/// The saved name a row is renamed from, if its name changed.
fn renamed_from(field: &FieldDef) -> Option<&str> {
    field
        .renamed_from
        .as_deref()
        .filter(|from| *from != field.name)
}

fn saved_key(name: &str) -> String {
    format!("saved:{name}")
}

/// Identifies a row's collapsed state: saved rows by the field they edit,
/// unsaved rows by name in a separate namespace. A collapsed row hides its
/// name input, so its name, and key, stay put until it is expanded. Only two
/// unsaved rows with the same (invalid, duplicate) name share a state.
fn collapse_key(field: &FieldDef, saved: &[String]) -> String {
    match row_origin(field, saved) {
        Some(origin) => saved_key(&origin),
        None => format!("new:{}", field.name),
    }
}

/// Saved stored fields whose values are deleted when `fields` is saved, as the
/// server decides: those no row keeps by name or renames, and those whose name
/// another saved field is renamed to.
fn removed_fields(saved: &[FieldDef], fields: &[FieldDef]) -> Vec<String> {
    let fields = strip_self_marks(fields);
    let renamed = |name: &str| {
        fields
            .iter()
            .any(|f| f.renamed_from.as_deref() == Some(name))
    };
    let kept = |name: &str| {
        fields
            .iter()
            .any(|f| f.name == name && f.renamed_from.is_none())
    };
    saved
        .iter()
        .filter(|f| f.kind != FieldType::Calculated)
        .filter(|f| !renamed(&f.name) && !kept(&f.name))
        .map(|f| f.name.clone())
        .collect()
}

/// Collapsed rows before any are toggled: saved fields, in long lists.
fn default_collapsed(saved: &[String]) -> BTreeSet<String> {
    if saved.len() > 5 {
        saved.iter().map(|name| saved_key(name)).collect()
    } else {
        Default::default()
    }
}

/// The collapsed keys for `after`, the rows `before` as reloaded after a save
/// (in the same order), so each row stays collapsed or open as it was. A row
/// the save created changes key from `new:{name}` to `saved:{name}`.
fn carry_collapsed(
    collapsed: &BTreeSet<String>,
    before: &[FieldDef],
    saved_before: &[String],
    after: &[FieldDef],
    saved_after: &[String],
) -> BTreeSet<String> {
    before
        .iter()
        .zip(after)
        .filter(|(row, _)| collapsed.contains(&collapse_key(row, saved_before)))
        .map(|(_, row)| collapse_key(row, saved_after))
        .collect()
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
    /// The saved field's allowed values.
    saved_options: Vec<String>,
    on_removed: Callback<String>,
    on_renamed: Callback<(String, String)>,
    on_change: Callback<Vec<FieldDef>>,
    /// Whether the field's settings are shown, or only its summary.
    open: bool,
    on_toggle: Callback<()>,
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
    let type_label = field_types()
        .into_iter()
        .find(|(value, _)| *value == type_value(field.kind))
        .map_or("", |(_, label)| label);
    let title = if field.name.is_empty() {
        "Unnamed field"
    } else {
        field.display_label()
    };
    let summary = html! {
        <div class="field-summary">
            <button type="button" class="field-toggle" aria-expanded={props.open.to_string()} onclick={props.on_toggle.reform(|_| ())}>
                <span class="field-chevron" aria-hidden="true">{"▸"}</span>
                <strong>{title.to_string()}</strong>
                if !field.display_label().eq_ignore_ascii_case(&field.name) && !field.name.is_empty() {
                    <code>{field.name.clone()}</code>
                }
                <span class="badge">{type_label}</span>
                if field.required {<span class="badge">{"Required"}</span>}
                if renamed_from(&field).is_some() {<span class="badge">{"Renamed"}</span>}
            </button>
            <div class="row field-actions">
                <button class="secondary" type="button" aria-label={format!("Move {title} up")} disabled={props.index == 0} onclick={Callback::from({
                    let move_by = move_by.clone();
                    move |_| move_by.emit(-1)
                })}>{"Up"}</button>
                <button class="secondary" type="button" aria-label={format!("Move {title} down")} disabled={props.index + 1 == props.total} onclick={Callback::from({
                    let move_by = move_by.clone();
                    move |_| move_by.emit(1)
                })}>{"Down"}</button>
                <button type="button" class="danger" aria-label={format!("Remove {title}")} onclick={remove}>{"Remove"}</button>
            </div>
        </div>
    };
    if !props.open {
        return html! { <div class="field-row collapsed">{summary}</div> };
    }
    html! {
        <div class="field-row">
            {summary}
            <label>{"Name"}
                <input aria-label="Name" value={field.name.clone()} onfocus={Callback::from({
                    let focused = focused_name.clone();
                    let name = field.name.clone();
                    move |_| *focused.borrow_mut() = Some(name.clone())
                })} oninput={Callback::from({
                    let patch = patch.clone();
                    let field = field.clone();
                    move |event: InputEvent| {
                        let element: HtmlInputElement = event.target_unchecked_into();
                        let mut next = field.clone();
                        // `renamed_from` keeps naming the saved field this row
                        // edits, so its data moves to the new name on save.
                        next.name = element.value().to_ascii_lowercase();
                        patch.emit(next);
                    }
                })} onchange={commit_name} />
                if let Some(from) = renamed_from(&field) {
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
                        if next.kind != FieldType::String {
                            next.options = None;
                            next.renamed_options = None;
                        }
                        if !matches!(next.kind, FieldType::Number | FieldType::Calculated) {
                            next.number_format = None;
                        }
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
            if field.kind == FieldType::String {
                <OptionsEditor field={field.clone()} saved={props.saved_options.clone()} records={props.records.clone()} origin={props.origin.clone()} on_change={patch.clone()}/>
            }
            if matches!(field.kind, FieldType::Number | FieldType::Calculated) {
                {number_format_editor(&field, &patch)}
            }
            if field.kind == FieldType::Calculated {
                <FormulaEditor records={props.records.clone()} field={field.clone()} fields={props.fields.clone()} collections={props.collections.clone()} current={props.current.clone()} on_change={patch.clone()}/>
            }
        </div>
    }
}

/// Locales offered for number formats, as tag and name.
const FORMAT_LOCALES: [(&str, &str); 6] = [
    ("nb-NO", "Norwegian (1 234 567,89)"),
    ("sv-SE", "Swedish (1 234 567,89)"),
    ("da-DK", "Danish (1.234.567,89)"),
    ("de-DE", "German (1.234.567,89)"),
    ("en-GB", "English UK (1,234,567.89)"),
    ("en-US", "English US (1,234,567.89)"),
];

/// Currencies offered for number formats; others can be typed.
const FORMAT_CURRENCIES: [&str; 6] = ["NOK", "SEK", "DKK", "EUR", "USD", "GBP"];

/// How a number or calculated field's numbers are shown: plain, grouped, or
/// as an amount in a currency. Display only; stored values stay numbers.
fn number_format_editor(field: &FieldDef, patch: &Callback<FieldDef>) -> Html {
    let format = field.number_format.clone();
    let style = match &format {
        None => "plain",
        Some(f) if f.currency.is_some() => "currency",
        Some(_) => "grouped",
    };
    let emit = {
        let (field, patch) = (field.clone(), patch.clone());
        move |format: Option<vulse_model::NumberFormat>| {
            let mut next = field.clone();
            next.number_format = format;
            patch.emit(next);
        }
    };
    let on_style = {
        let (emit, format) = (emit.clone(), format.clone());
        Callback::from(move |event: Event| {
            let element: HtmlSelectElement = event.target_unchecked_into();
            let mut next = format.clone().unwrap_or_default();
            match element.value().as_str() {
                "plain" => return emit(None),
                "currency" => {
                    next.currency.get_or_insert_with(|| "NOK".into());
                    next.locale.get_or_insert_with(|| "nb-NO".into());
                }
                _ => next.currency = None,
            }
            emit(Some(next));
        })
    };
    let update = {
        let (emit, format) = (emit.clone(), format.clone());
        move |change: fn(&mut vulse_model::NumberFormat, String)| {
            let (emit, format) = (emit.clone(), format.clone());
            Callback::from(move |event: Event| {
                let element: HtmlSelectElement = event.target_unchecked_into();
                let mut next = format.clone().unwrap_or_default();
                change(&mut next, element.value());
                emit(Some(next));
            })
        }
    };
    html! {
        <div class="field-options number-format">
            <label>{"Number display"}
                <select aria-label="Number display" onchange={on_style}>
                    <option value="plain" selected={style == "plain"}>{"Plain (1234567.5)"}</option>
                    <option value="grouped" selected={style == "grouped"}>{"Formatted number (1 234 567,5)"}</option>
                    <option value="currency" selected={style == "currency"}>{"Currency (1 234 567,50 kr)"}</option>
                </select>
            </label>
            if let Some(current) = &format {
                <div class="field-metadata">
                    if let Some(code) = &current.currency {
                        <label>{"Currency"}
                            <select aria-label="Currency" onchange={update(|f, value| f.currency = Some(value))}>
                                { for FORMAT_CURRENCIES.iter().map(|c| html! { <option value={*c} selected={code == c}>{*c}</option> }) }
                                if !FORMAT_CURRENCIES.contains(&code.as_str()) {
                                    <option value={code.clone()} selected=true>{code.clone()}</option>
                                }
                            </select>
                        </label>
                    }
                    <label>{"Decimals"}
                        <select aria-label="Decimals" onchange={update(|f, value| f.decimals = value.parse().ok())}>
                            <option value="" selected={current.decimals.is_none()}>{if current.currency.is_some() {"Currency default"} else {"As stored"}}</option>
                            { for (0..=vulse_model::MAX_FORMAT_DECIMALS).map(|d| html! { <option value={d.to_string()} selected={current.decimals == Some(d)}>{d.to_string()}</option> }) }
                        </select>
                    </label>
                    <label>{"Separators"}
                        <select aria-label="Separators" onchange={update(|f, value| f.locale = (!value.is_empty()).then_some(value))}>
                            <option value="" selected={current.locale.is_none()}>{"Viewer's browser language"}</option>
                            { for FORMAT_LOCALES.iter().map(|(tag, name)| html! { <option value={*tag} selected={current.locale.as_deref() == Some(*tag)}>{*name}</option> }) }
                            if let Some(tag) = current.locale.as_deref().filter(|tag| !FORMAT_LOCALES.iter().any(|(t, _)| t == tag)) {
                                <option value={tag.to_string()} selected=true>{tag.to_string()}</option>
                            }
                        </select>
                    </label>
                </div>
                <p class="muted">{format!("Example: {}. Only the admin's display changes; the API, filters, imports and exports keep plain numbers.", crate::number_format::format_number(1234567.5, current))}</p>
            }
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct OptionsEditorProps {
    field: FieldDef,
    /// The saved field's allowed values.
    saved: Vec<String>,
    records: Vec<DocumentView>,
    origin: Option<String>,
    on_change: Callback<FieldDef>,
}

#[derive(Clone, PartialEq)]
struct OptionSlot {
    key: u64,
    origin: Option<String>,
}

fn derive_option_slots(
    saved: &[String],
    options: &[String],
    renames: &std::collections::BTreeMap<String, String>,
    seq: &mut u64,
) -> Vec<OptionSlot> {
    let mut claimed = std::collections::HashSet::new();
    options
        .iter()
        .map(|value| {
            let origin = saved
                .iter()
                .find(|s| !claimed.contains(*s) && *s == value && !renames.contains_key(*s))
                .cloned()
                .or_else(|| {
                    renames
                        .iter()
                        .find(|(from, to)| !claimed.contains(*from) && *to == value)
                        .map(|(from, _)| from.clone())
                });
            if let Some(from) = &origin {
                claimed.insert(from.clone());
            }
            *seq += 1;
            OptionSlot { key: *seq, origin }
        })
        .collect()
}

fn compute_option_renames(
    slots: &[OptionSlot],
    options: &[String],
) -> std::collections::BTreeMap<String, String> {
    let mut renames = std::collections::BTreeMap::new();
    for (slot, value) in slots.iter().zip(options) {
        if let Some(from) = &slot.origin
            && from != value
            && !options.contains(from)
        {
            renames.insert(from.clone(), value.clone());
        }
    }
    renames
}

/// Allowed values of a text field. Editing a saved value renames it: records
/// holding it get the new value when the fields are saved.
#[function_component(OptionsEditor)]
fn options_editor(props: &OptionsEditorProps) -> Html {
    let field = &props.field;
    let options = field.options.clone().unwrap_or_default();
    let renames = field.renamed_options.clone().unwrap_or_default();
    let seq = use_mut_ref(|| 0u64);
    let slots = use_mut_ref(Vec::<OptionSlot>::new);
    let saved_snapshot = use_mut_ref(Vec::<String>::new);
    // The options and renames this editor last showed or sent to its parent.
    let expected =
        use_mut_ref(|| None::<(Vec<String>, std::collections::BTreeMap<String, String>)>);
    {
        let mut s = slots.borrow_mut();
        let mut saved_prev = saved_snapshot.borrow_mut();
        if field.options.is_none() {
            s.clear();
            *saved_prev = props.saved.clone();
        } else if s.len() != options.len()
            || *saved_prev != props.saved
            || expected.borrow().as_ref() != Some(&(options.clone(), renames.clone()))
        {
            // Also when the options were replaced from outside this editor
            // (a restored or discarded draft), which leaves the slots stale.
            let mut next_seq = *seq.borrow();
            *s = derive_option_slots(&props.saved, &options, &renames, &mut next_seq);
            *seq.borrow_mut() = next_seq;
            *saved_prev = props.saved.clone();
            *expected.borrow_mut() = Some((options.clone(), renames.clone()));
        }
    }
    let current_slots = slots.borrow().clone();
    let emit = {
        let field = field.clone();
        let on_change = props.on_change.clone();
        let expected = expected.clone();
        move |options: Option<Vec<String>>, renames: std::collections::BTreeMap<String, String>| {
            *expected.borrow_mut() = Some((options.clone().unwrap_or_default(), renames.clone()));
            let mut next = field.clone();
            next.options = options;
            next.renamed_options = (!renames.is_empty()).then_some(renames);
            on_change.emit(next);
        }
    };
    let toggle = {
        let emit = emit.clone();
        let records = props.records.clone();
        let origin = props.origin.clone();
        let saved = props.saved.clone();
        let slots = slots.clone();
        let seq = seq.clone();
        Callback::from(move |event: Event| {
            let input: HtmlInputElement = event.target_unchecked_into();
            if !input.checked() {
                slots.borrow_mut().clear();
                emit(None, Default::default());
                return;
            }
            // Start from saved options or the values loaded records already hold.
            let mut values: Vec<String> = saved.clone();
            for record in &records {
                if let Some(value) = origin
                    .as_ref()
                    .and_then(|name| record.body.get(name))
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    && !values.iter().any(|v| v.eq_ignore_ascii_case(value))
                    && values.len() < vulse_model::MAX_FIELD_OPTIONS
                {
                    values.push(value.to_string());
                }
            }
            if values.is_empty() {
                values.push(String::new());
            }
            let mut next_seq = *seq.borrow();
            *slots.borrow_mut() =
                derive_option_slots(&saved, &values, &Default::default(), &mut next_seq);
            *seq.borrow_mut() = next_seq;
            emit(Some(values), Default::default());
        })
    };
    let removed: Vec<String> = props
        .saved
        .iter()
        .filter(|value| !options.contains(value) && !renames.contains_key(*value))
        .cloned()
        .collect();
    html! {
        <div class="field-options">
            <label class="check">
                <input type="checkbox" checked={field.options.is_some()} onchange={toggle} />
                {"Limit to allowed values (shown as a dropdown)"}
            </label>
            if field.options.is_some() {
                <ol class="option-list">
                    { for options.iter().enumerate().map(|(index, value)| {
                        let slot = current_slots.get(index).cloned().unwrap_or(OptionSlot { key: index as u64, origin: None });
                        let origin = slot.origin.clone();
                        let edit = {
                            let (options, slots_ref, emit) = (options.clone(), slots.clone(), emit.clone());
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                let text = input.value();
                                let mut options = options.clone();
                                options[index] = text;
                                let renames = compute_option_renames(&slots_ref.borrow(), &options);
                                emit(Some(options), renames);
                            })
                        };
                        let shift = |delta: isize| {
                            let (options, slots_ref, emit) = (options.clone(), slots.clone(), emit.clone());
                            Callback::from(move |_| {
                                let target = index as isize + delta;
                                if target >= 0 && (target as usize) < options.len() {
                                    let mut options = options.clone();
                                    options.swap(index, target as usize);
                                    slots_ref.borrow_mut().swap(index, target as usize);
                                    let renames = compute_option_renames(&slots_ref.borrow(), &options);
                                    emit(Some(options), renames);
                                }
                            })
                        };
                        let remove = {
                            let (options, slots_ref, emit) = (options.clone(), slots.clone(), emit.clone());
                            Callback::from(move |_| {
                                let mut options = options.clone();
                                options.remove(index);
                                let mut s = slots_ref.borrow_mut();
                                if index < s.len() {
                                    s.remove(index);
                                }
                                let renames = compute_option_renames(&s, &options);
                                drop(s);
                                emit(Some(options), renames);
                            })
                        };
                        let renamed_from = origin.filter(|from| from != value && !options.contains(from));
                        html! {
                            <li key={slot.key}>
                                <div class="row">
                                    <input aria-label={format!("Allowed value {}", index + 1)} maxlength={vulse_model::MAX_OPTION_LENGTH.to_string()} value={value.clone()} oninput={edit} />
                                    <button type="button" class="secondary" aria-label={format!("Move allowed value {} up", index + 1)} disabled={index == 0} onclick={shift(-1)}>{"↑"}</button>
                                    <button type="button" class="secondary" aria-label={format!("Move allowed value {} down", index + 1)} disabled={index + 1 == options.len()} onclick={shift(1)}>{"↓"}</button>
                                    <button type="button" class="text-button" aria-label={format!("Remove allowed value {}", index + 1)} onclick={remove}>{"Remove"}</button>
                                </div>
                                if let Some(from) = renamed_from {
                                    <span class="field-help">{format!("Renamed from “{from}”. Records with “{from}” get the new value when you save.")}</span>
                                }
                            </li>
                        }
                    }) }
                </ol>
                <button type="button" class="secondary" disabled={options.len() >= vulse_model::MAX_FIELD_OPTIONS} onclick={Callback::from({
                    let (options, slots_ref, seq, emit) = (options.clone(), slots.clone(), seq.clone(), emit.clone());
                    move |_| {
                        let mut options = options.clone();
                        options.push(String::new());
                        let mut next_seq = *seq.borrow() + 1;
                        *seq.borrow_mut() = next_seq;
                        let mut s = slots_ref.borrow_mut();
                        s.push(OptionSlot { key: next_seq, origin: None });
                        let renames = compute_option_renames(&s, &options);
                        drop(s);
                        let _ = &mut next_seq;
                        emit(Some(options), renames);
                    }
                })}>{"Add value"}</button>
                if !removed.is_empty() {
                    <p class="field-help">{format!("Removed: {}. Saving is refused while records still use a removed value; change those records first, or rename the value instead.", removed.iter().map(|v| format!("“{v}”")).collect::<Vec<_>>().join(", "))}</p>
                }
                <p class="muted">{"Values must match exactly. Editing a saved value renames it in every record and saved view filter when you save. Earlier history versions keep the old value."}</p>
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, kind: FieldType) -> FieldDef {
        FieldDef {
            label: None,
            description: None,
            example: None,
            name: name.into(),
            required: false,
            kind,
            collection: None,
            items: None,
            formula: None,
            renamed_from: None,
            number_format: None,
            options: None,
            renamed_options: None,
        }
    }

    fn names(fields: &[FieldDef]) -> Vec<String> {
        fields.iter().map(|f| f.name.clone()).collect()
    }

    #[test]
    fn typing_a_new_fields_name_through_a_saved_name_keeps_both_identities() {
        let saved = vec![field("price", FieldType::Number)];
        let saved_names = names(&saved);
        let mut rows = mark_saved(&saved);
        rows.push(field("field_2", FieldType::String));
        // The Name input only sets the name, one keystroke at a time.
        for typed in ["", "p", "pr", "price", "price_", "price_total"] {
            rows[1].name = typed.into();
            assert_eq!(rows[1].renamed_from, None, "{typed}");
            assert_eq!(row_origin(&rows[1], &saved_names), None, "{typed}");
            assert_eq!(row_origin(&rows[0], &saved_names).as_deref(), Some("price"));
            assert_eq!(renamed_from(&rows[0]), None);
            assert_eq!(renamed_from(&rows[1]), None);
            // A collapsed saved row never collapses the row being typed in.
            assert_ne!(
                collapse_key(&rows[1], &saved_names),
                collapse_key(&rows[0], &saved_names)
            );
        }
        assert!(removed_fields(&saved, &rows).is_empty());
        let sent = strip_self_marks(&rows);
        assert!(sent.iter().all(|f| f.renamed_from.is_none()));
        assert_eq!(names(&sent), ["price", "price_total"]);
    }

    #[test]
    fn renaming_a_saved_field_keeps_its_origin_and_key() {
        let saved = vec![field("price", FieldType::Number)];
        let saved_names = names(&saved);
        let mut rows = mark_saved(&saved);
        let key = collapse_key(&rows[0], &saved_names);
        rows[0].name = "cost".into();
        assert_eq!(row_origin(&rows[0], &saved_names).as_deref(), Some("price"));
        assert_eq!(renamed_from(&rows[0]), Some("price"));
        assert_eq!(collapse_key(&rows[0], &saved_names), key);
        assert_eq!(
            strip_self_marks(&rows)[0].renamed_from.as_deref(),
            Some("price")
        );
        assert!(removed_fields(&saved, &rows).is_empty());
        // Back to its saved name, it is no longer a rename.
        rows[0].name = "price".into();
        assert_eq!(renamed_from(&rows[0]), None);
        assert_eq!(strip_self_marks(&rows), saved);
    }

    #[test]
    fn legacy_drafts_adopt_unclaimed_saved_names_once() {
        let saved = names(&[
            field("title", FieldType::String),
            field("price", FieldType::Number),
            field("memo", FieldType::String),
        ]);
        let mut renamed = field("cost", FieldType::Number);
        renamed.renamed_from = Some("price".into());
        // Written before self-markers: only the rename is marked. The new
        // `price` row is not the saved `price`, which `cost` edits; of two
        // rows named `title` only the first is the saved one.
        let draft = vec![
            field("title", FieldType::String),
            renamed,
            field("price", FieldType::String),
            field("title", FieldType::String),
            field("extra", FieldType::String),
        ];
        let rows = adopt_saved_rows(draft, &saved);
        let origins: Vec<Option<String>> = rows.iter().map(|f| row_origin(f, &saved)).collect();
        assert_eq!(
            origins,
            [Some("title".into()), Some("price".into()), None, None, None]
        );
        // A draft with self-markers is restored as written.
        let marked = mark_saved(&[field("title", FieldType::String)]);
        assert_eq!(adopt_saved_rows(marked.clone(), &saved), marked);
    }

    #[test]
    fn stale_markers_edit_nothing() {
        let rows = mark_saved(&[field("gone", FieldType::String)]);
        assert_eq!(row_origin(&rows[0], &["other".into()]), None);
        assert_eq!(collapse_key(&rows[0], &["other".into()]), "new:gone");
        assert_eq!(strip_self_marks(&rows)[0].renamed_from, None);
    }

    #[test]
    fn option_slots_keep_row_origins_independent_of_typed_values() {
        let saved = vec!["Active".to_string(), "Draft".into()];
        let mut seq = 0u64;
        let mut options = saved.clone();
        let mut slots = derive_option_slots(&saved, &options, &Default::default(), &mut seq);
        // Add a new option and type through `"Active"` to `"Active+"`.
        options.push(String::new());
        seq += 1;
        slots.push(OptionSlot {
            key: seq,
            origin: None,
        });
        options[2] = "Active".into();
        assert!(compute_option_renames(&slots, &options).is_empty());
        options[2] = "Active+".into();
        assert!(compute_option_renames(&slots, &options).is_empty());

        // Clearing `"Draft"` to `""` while another blank row is removed keeps `"Draft"`'s origin.
        options.push(String::new());
        seq += 1;
        slots.push(OptionSlot {
            key: seq,
            origin: None,
        });
        options[1] = String::new();
        options.pop();
        slots.pop();
        options[1] = "Pending".into();
        let renames = compute_option_renames(&slots, &options);
        assert_eq!(renames.get("Draft").map(String::as_str), Some("Pending"));
    }

    #[test]
    fn renaming_onto_a_saved_name_removes_the_replaced_field() {
        let saved = vec![
            field("a", FieldType::Number),
            field("b", FieldType::String),
            field("c", FieldType::Calculated),
        ];
        let mut rows = mark_saved(&saved[..1]);
        rows[0].name = "b".into();
        assert_eq!(removed_fields(&saved, &rows), ["b"]);
        // Plain removals still count, calculated fields never do.
        assert_eq!(removed_fields(&saved, &[]), ["a", "b"]);
        assert_eq!(
            removed_fields(&saved, &mark_saved(&saved)),
            Vec::<String>::new()
        );
    }

    fn keys(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn rows_stay_open_or_collapsed_across_a_save() {
        // Five saved fields start open; a sixth, added and saved, would make
        // them all start collapsed, the new row included. Nothing was
        // toggled, so every row stays open.
        let saved: Vec<FieldDef> = ["a", "b", "c", "d", "e"]
            .map(|name| field(name, FieldType::String))
            .to_vec();
        let mut rows = mark_saved(&saved);
        rows.push(field("f", FieldType::String));
        // The server returns the fields as sent, in order; reloading marks them.
        let after = mark_saved(&rows);
        let before_names = names(&saved);
        let after_names = names(&after);
        assert!(default_collapsed(&after_names).contains("saved:f"));
        let was = default_collapsed(&before_names);
        assert!(was.is_empty());
        assert!(carry_collapsed(&was, &rows, &before_names, &after, &after_names).is_empty());

        // In a long list: a renamed saved row the user opened stays open, a
        // collapsed new row stays collapsed, an open new row stays open.
        let saved: Vec<FieldDef> = ["a", "b", "c", "d", "e", "f"]
            .map(|name| field(name, FieldType::String))
            .to_vec();
        let mut rows = mark_saved(&saved);
        rows[0].name = "a2".into();
        rows.push(field("g", FieldType::String));
        rows.push(field("h", FieldType::String));
        let collapsed = keys(&[
            "saved:b", "saved:c", "saved:d", "saved:e", "saved:f", "new:g",
        ]);
        let after = mark_saved(&rows);
        assert_eq!(names(&after), ["a2", "b", "c", "d", "e", "f", "g", "h"]);
        assert_eq!(
            carry_collapsed(&collapsed, &rows, &names(&saved), &after, &names(&after)),
            keys(&[
                "saved:b", "saved:c", "saved:d", "saved:e", "saved:f", "saved:g"
            ])
        );
    }
}
