use serde_json::json;
use vulse_model::{CollectionView, DocumentView, DocumentWrite};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::app::{Route, SessionCtx, Shell, signed_out};
use crate::confirm::{ConfirmRequest, use_confirm};
use crate::form::{DocumentForm, ValueInput, body_from_inputs, inputs_from_body};

#[derive(Properties, PartialEq)]
pub struct DocumentPageProps {
    pub id: String,
    pub doc_id: String,
}

#[function_component(DocumentPage)]
pub fn document_page(props: &DocumentPageProps) -> Html {
    let confirm = use_confirm();
    let session = use_context::<SessionCtx>().expect("session");
    let collection = use_state(|| None::<CollectionView>);
    let collections = use_state(Vec::<CollectionView>::new);
    let inputs = use_state(Vec::<ValueInput>::new);
    let ready = use_state(|| false);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let status = use_state(String::new);
    let baseline = use_state(Vec::<ValueInput>::new);
    let version = use_state(|| None::<String>);
    let current = use_state(|| None::<DocumentView>);
    let pending = use_state(|| None::<crate::drafts::RecordDraft>);
    let storage_error = use_state(String::new);
    let draft_key = format!(
        "vulse:draft:record:{}:{}:{}",
        session
            .state
            .user
            .as_ref()
            .map(|u| u.id.as_str())
            .unwrap_or("signedout"),
        props.id,
        props.doc_id
    );
    let latest = use_mut_ref(Vec::<ValueInput>::new);
    *latest.borrow_mut() = (*inputs).clone();
    let dirty = *inputs != *baseline;
    let creating = props.doc_id == "new";
    {
        let id = props.id.clone();
        let doc_id = props.doc_id.clone();
        let collection = collection.clone();
        let collections = collections.clone();
        let inputs = inputs.clone();
        let ready = ready.clone();
        let error = error.clone();
        let baseline = baseline.clone();
        let version = version.clone();
        let current = current.clone();
        let pending = pending.clone();
        let storage_error = storage_error.clone();
        let draft_key = draft_key.clone();
        let authed = session.state.user.is_some();
        use_effect_with(
            (id.clone(), doc_id.clone(), authed),
            move |(id, doc_id, authed)| {
                if !*authed {
                    return;
                }
                ready.set(false);
                pending.set(None);
                current.set(None);
                version.set(None);
                error.set(String::new());
                let id = id.clone();
                let doc_id = doc_id.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let loaded = match api::collection(&id).await {
                        Ok(item) => item,
                        Err(err) => {
                            error.set(err);
                            return;
                        }
                    };
                    if let Ok(items) = api::collections().await {
                        collections.set(items);
                    }
                    let body = if doc_id == "new" {
                        json!({})
                    } else {
                        match api::document(&id, &doc_id).await {
                            Ok(document) => {
                                version.set(Some(document.updated_at.clone()));
                                current.set(Some(document.clone()));
                                document.body
                            }
                            Err(err) => {
                                error.set(err);
                                return;
                            }
                        }
                    };
                    match inputs_from_body(&loaded.blueprint, &body) {
                        Ok(next) => {
                            baseline.set(next.clone());
                            inputs.set(next);
                            match crate::drafts::read(&draft_key) {
                                Ok(draft) => pending.set(draft),
                                Err(e) => storage_error.set(e),
                            }
                            collection.set(Some(loaded));
                            ready.set(true);
                        }
                        Err(err) => error.set(err),
                    }
                });
            },
        );
    }
    {
        let key = draft_key.clone();
        let collection = (*collection).clone();
        let storage_error = storage_error.clone();
        use_effect_with(
            (
                (*inputs).clone(),
                (*baseline).clone(),
                (*version).clone(),
                *ready,
                pending.is_some(),
            ),
            move |(inputs, baseline, version, ready, pending)| {
                if *ready && !*pending && inputs != baseline {
                    if let Some(c) = collection {
                        match crate::drafts::write(
                            &key,
                            &crate::drafts::RecordDraft {
                                blueprint: c.blueprint,
                                inputs: inputs.clone(),
                                baseline: baseline.clone(),
                                version: version.clone(),
                            },
                        ) {
                            Ok(()) => storage_error.set(String::new()),
                            Err(e) => storage_error.set(e),
                        }
                    }
                }
            },
        );
    }
    let discard = Callback::from({
        let pending = pending.clone();
        let key = draft_key.clone();
        let storage_error = storage_error.clone();
        let confirm = confirm.clone();
        move |_| {
            let pending = pending.clone();
            let key = key.clone();
            let storage_error = storage_error.clone();
            let confirm = confirm.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let request = ConfirmRequest::danger(
                    "Discard this draft?",
                    "The unsaved changes stored on this device are deleted. The saved record is not affected.",
                    "Discard draft",
                );
                if !confirm.ask(request).await {
                    return;
                }
                match crate::drafts::clear(&key) {
                    Ok(()) => {
                        pending.set(None);
                        storage_error.set(String::new());
                    }
                    Err(e) => storage_error.set(e),
                }
            });
        }
    });
    let recover = Callback::from({
        let pending = pending.clone();
        let inputs = inputs.clone();
        let baseline = baseline.clone();
        let version = version.clone();
        let collection = collection.clone();
        let error = error.clone();
        move |_| {
            let (Some(draft), Some(collection)) = ((*pending).clone(), (*collection).clone())
            else {
                return;
            };
            let compatible = draft.blueprint.fields.len() == collection.blueprint.fields.len()
                && draft
                    .blueprint
                    .fields
                    .iter()
                    .zip(&collection.blueprint.fields)
                    .all(|(a, b)| {
                        a.name == b.name
                            && a.kind == b.kind
                            && a.items == b.items
                            && a.collection == b.collection
                    });
            if !compatible {
                error.set("The collection fields changed. The draft is kept on this device; copy needed values before discarding it.".into());
                return;
            }
            inputs.set(draft.inputs);
            baseline.set(draft.baseline);
            version.set(draft.version);
            pending.set(None);
        }
    });
    let merge = Callback::from({
        let pending = pending.clone();
        let inputs = inputs.clone();
        let collection = collection.clone();
        let error = error.clone();
        let confirm = confirm.clone();
        move |_| {
            let (Some(draft), Some(collection)) = ((*pending).clone(), (*collection).clone())
            else {
                return;
            };
            let pending = pending.clone();
            let inputs = inputs.clone();
            let error = error.clone();
            let confirm = confirm.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let request = ConfirmRequest::new(
                    "Merge your draft?",
                    "Your changed draft fields are applied to the latest record. Where both changed the same field, your draft value wins. Review, then save.",
                    "Merge draft",
                );
                if !confirm.ask(request).await {
                    return;
                }
                let mut next = (*inputs).clone();
                for (index, field) in collection.blueprint.fields.iter().enumerate() {
                    if let Some(old) = draft.blueprint.fields.iter().position(|f| {
                        f.name == field.name
                            && f.kind == field.kind
                            && f.items == field.items
                            && f.collection == field.collection
                    }) {
                        if draft.inputs.get(old) != draft.baseline.get(old) {
                            if let Some(value) = draft.inputs.get(old) {
                                next[index] = value.clone();
                            }
                        }
                    }
                }
                inputs.set(next);
                pending.set(None);
                error.set(String::new());
            });
        }
    });
    let navigator = use_navigator();
    let save_navigator = navigator.clone();
    let on_save = {
        let id = props.id.clone();
        let doc_id = props.doc_id.clone();
        let collection = collection.clone();
        let inputs = inputs.clone();
        let error = error.clone();
        let navigator = save_navigator;
        let busy = busy.clone();
        let status = status.clone();
        let version = version.clone();
        let baseline = baseline.clone();
        let current = current.clone();
        let key = draft_key.clone();
        let latest = latest.clone();
        let storage_error = storage_error.clone();
        Callback::from(move |_| {
            if *busy {
                return;
            }
            let Some(collection) = (*collection).clone() else {
                return;
            };
            let body = match body_from_inputs(&collection.blueprint, &inputs) {
                Ok(body) => body,
                Err(err) => {
                    error.set(err);
                    return;
                }
            };
            let submitted = (*inputs).clone();
            let expected = (*version).clone();
            let (version, baseline, current, key, latest, storage_error) = (
                version.clone(),
                baseline.clone(),
                current.clone(),
                key.clone(),
                latest.clone(),
                storage_error.clone(),
            );
            busy.set(true);
            status.set(String::new());
            let busy = busy.clone();
            let status = status.clone();
            let id = id.clone();
            let doc_id = doc_id.clone();
            let error = error.clone();
            let navigator = navigator.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let write = DocumentWrite {
                    body,
                    expected_updated_at: expected,
                };
                let result = if doc_id == "new" {
                    api::create_document(&id, &write).await
                } else {
                    api::update_document(&id, &doc_id, &write).await
                };
                match result {
                    Ok(document) => {
                        error.set(String::new());
                        version.set(Some(document.updated_at.clone()));
                        baseline.set(submitted.clone());
                        current.set(Some(document.clone()));
                        if *latest.borrow() == submitted {
                            if let Err(e) = crate::drafts::clear(&key) {
                                storage_error.set(e);
                            }
                        }
                        status.set("Record saved".into());
                        if doc_id == "new" {
                            if let Some(navigator) = navigator {
                                navigator.push(&Route::Document {
                                    id,
                                    doc_id: document.id,
                                });
                            }
                        }
                    }
                    Err(err) => error.set(err),
                }
                busy.set(false);
            });
        })
    };
    let on_delete = {
        let id = props.id.clone();
        let doc_id = props.doc_id.clone();
        let error = error.clone();
        let navigator = navigator.clone();
        let confirm = confirm.clone();
        Callback::from(move |_| {
            let id = id.clone();
            let doc_id = doc_id.clone();
            let error = error.clone();
            let navigator = navigator.clone();
            let confirm = confirm.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let request = ConfirmRequest::danger(
                    "Delete this record?",
                    "You can restore it later from the collection’s Trash.",
                    "Delete record",
                );
                if !confirm.ask(request).await {
                    return;
                }
                match api::delete_document(&id, &doc_id).await {
                    Ok(()) => {
                        if let Some(navigator) = navigator {
                            navigator.push(&Route::Collection { id });
                        }
                    }
                    Err(err) => error.set(err),
                }
            });
        })
    };
    if let Some(redirect) = signed_out(&session) {
        return redirect;
    }
    html! {
        <Shell>
            <p class="muted">
                <Link<Route> to={Route::Collection { id: props.id.clone() }}>{"Back to collection"}</Link<Route>>
            </p>
            <div class="page-heading"><div><p class="eyebrow">{"RECORD EDITOR"}</p><h1>{ if creating { "New record".into() } else { collection.as_ref().map(|c|vulse_model::record_title(&c.blueprint,&body_from_inputs(&c.blueprint,&inputs).unwrap_or_else(|_|json!({})),&props.doc_id)).unwrap_or_else(||"Edit record".into()) } }</h1><p class="muted">{"Add the details below. Calculated values update as you type."}</p></div></div>
            if pending.is_some(){<section class="card draft-banner"><strong>{"A draft is saved on this device"}</strong><p>{"Restore it to continue editing, or apply only its changed fields to the latest record. Saving an older draft will check for conflicts."}</p><div class="row"><button class="secondary" onclick={recover}>{"Restore draft"}</button><button class="secondary" onclick={merge}>{"Merge draft onto latest"}</button><button class="danger" onclick={discard.clone()}>{"Discard draft"}</button></div><details><summary>{"View draft values"}</summary><pre>{pending.as_ref().and_then(|d|crate::form::body_from_inputs(&d.blueprint,&d.inputs).ok()).map(|v|serde_json::to_string_pretty(&v).unwrap_or_default()).unwrap_or_else(||serde_json::to_string_pretty(&*pending).unwrap_or_default())}</pre></details></section>}
            if !storage_error.is_empty(){<p class="error" role="alert">{(*storage_error).clone()}</p><button class="secondary" onclick={discard}>{"Discard unreadable draft"}</button>}
            if *ready {<p class="muted" role="status">{if dirty {if storage_error.is_empty(){"Unsaved changes · draft saved on this device"}else{"Unsaved changes · draft storage unavailable"}}else if creating{"New record · not saved yet"}else{"All changes saved"}}</p>}
            if *ready {
                if let Some(collection) = (*collection).clone() {
                    <fieldset disabled={pending.is_some()||(creating&&*busy)} class="draft-fields"><div inert={pending.is_some()||(creating&&*busy)}><DocumentForm
                        collection_id={collection.id.clone()}
                        blueprint={collection.blueprint.clone()}
                        collections={(*collections).clone()}
                        inputs={(*inputs).clone()}
                        saved={(*current).as_ref().map(|d| d.body.clone())}
                        on_change={Callback::from({
                            let inputs = inputs.clone();let latest=latest.clone();
                            move |next:Vec<ValueInput>|{*latest.borrow_mut()=next.clone();inputs.set(next)}
                        })}
                    /></div></fieldset>
                }
            } else if error.is_empty() {
                <p class="muted">{"Loading…"}</p>
            }
            if !error.is_empty() {
                <p class="error" role="alert">{(*error).clone()}</p>
            }
            if !status.is_empty() {<p class="success" role="status">{(*status).clone()}</p>}
            if !creating {if let Some(doc)=current.as_ref(){<crate::relationships::Backlinks collection_id={props.id.clone()} document_id={props.doc_id.clone()} version={doc.updated_at.clone()}/>}}
            if !creating && pending.is_none() {if let Some(c)=(*collection).clone(){<crate::history::HistoryPanel collection={c} document={(*current).clone()} {dirty} on_restore={Callback::from({let inputs=inputs.clone();let baseline=baseline.clone();let current=current.clone();let version=version.clone();let collection=collection.clone();let latest=latest.clone();let key=draft_key.clone();let error=error.clone();let submitted=(*inputs).clone();move |doc:DocumentView|{let Some(c)=collection.as_ref()else{return;};if let Ok(next)=inputs_from_body(&c.blueprint,&doc.body){if *latest.borrow()==submitted{inputs.set(next.clone());let _=crate::drafts::clear(&key);}baseline.set(next);version.set(Some(doc.updated_at.clone()));current.set(Some(doc));error.set(String::new());}}})}/>}}
            <div class="row sticky-actions">
                <button type="button" onclick={on_save} disabled={!*ready || *busy||pending.is_some()}>{if *busy {"Saving…"}else{"Save record"}}</button>
                <Link<Route> classes="button secondary" to={Route::Collection{id:props.id.clone()}}>{"Back to records"}</Link<Route>>
                if !creating {
                    <button type="button" class="danger" onclick={on_delete}>{"Delete record…"}</button>
                }
            </div>
        </Shell>
    }
}
