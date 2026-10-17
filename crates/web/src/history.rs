use crate::{
    api,
    app::Route,
    confirm::{ConfirmRequest, use_confirm},
};
use vulse_model::{CollectionView, DocumentRevision, DocumentView};
use yew::prelude::*;
use yew_router::prelude::*;
#[derive(Properties, PartialEq)]
pub struct HistoryProps {
    pub collection: CollectionView,
    #[prop_or_default]
    pub document: Option<DocumentView>,
    #[prop_or_default]
    pub on_restore: Callback<DocumentView>,
    #[prop_or(false)]
    pub dirty: bool,
}
#[function_component(HistoryPanel)]
pub fn history_panel(props: &HistoryProps) -> Html {
    let confirm = use_confirm();
    let revisions = use_state(Vec::<DocumentRevision>::new);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let offset = use_state(|| 0u32);
    let more = use_state(|| false);
    let refresh = use_state(|| 0usize);
    let epoch = use_mut_ref(|| 0usize);
    {
        let revisions = revisions.clone();
        let error = error.clone();
        let more = more.clone();
        let collection = props.collection.id.clone();
        let doc = props.document.as_ref().map(|d| d.id.clone());
        let version = props.document.as_ref().map(|d| d.updated_at.clone());
        let epoch = epoch.clone();
        use_effect_with(
            (collection, doc, version, *offset, *refresh),
            move |(id, doc, _, offset, _)| {
                *epoch.borrow_mut() += 1;
                let generation = *epoch.borrow();
                let (id, doc, offset) = (id.clone(), doc.clone(), *offset);
                wasm_bindgen_futures::spawn_local(async move {
                    let result = if let Some(doc) = doc {
                        api::revisions(&id, &doc, offset).await
                    } else {
                        api::deleted_records(&id, offset).await
                    };
                    if *epoch.borrow() != generation {
                        return;
                    }
                    match result {
                        Ok(page) => {
                            more.set(page.items.len() == 20);
                            revisions.set(page.items);
                            error.set(String::new());
                        }
                        Err(e) => error.set(e),
                    }
                });
            },
        );
    }
    html! {<section class="card history-panel"><h2>{if props.document.is_some(){"Revision history"}else{"Trash"}}</h2><p class="muted">{"Review a saved version before restoring it. Restores use the current fields and formulas; unavailable references or files must be corrected first."}</p>
    if !error.is_empty(){<p class="error" role="alert">{(*error).clone()}</p>}
    if revisions.is_empty(){<p class="muted">{"No versions to show."}</p>}
    {for revisions.iter().map(|revision|{let title=vulse_model::record_title(&revision.blueprint,&revision.body,&revision.document_id);let restore={let id=props.collection.id.clone();let rev=revision.clone();let expected=props.document.as_ref().map(|d|d.updated_at.clone());let busy=busy.clone();let error=error.clone();let on_restore=props.on_restore.clone();let refresh=refresh.clone();let dirty=props.dirty;let confirm=confirm.clone();let deleted=props.document.is_none();Callback::from(move |_|{if *busy{return;}let request=if dirty{ConfirmRequest::danger("Restore this version?","Your unsaved edits are replaced by this version. The current saved version stays in the history.","Replace and restore")}else if deleted{ConfirmRequest::new("Restore this record?","The record returns to the collection with this version's values.","Restore record")}else{ConfirmRequest::new("Restore this version?","The record's current values are replaced by this version. The current version stays in the history.","Restore version")};let(id,rev,expected,busy,error,on_restore,refresh,confirm)=(id.clone(),rev.clone(),expected.clone(),busy.clone(),error.clone(),on_restore.clone(),refresh.clone(),confirm.clone());wasm_bindgen_futures::spawn_local(async move{if !confirm.ask(request).await{return;}busy.set(true);match api::restore_revision(&id,&rev.document_id,&rev.id,expected).await{Ok(doc)=>{on_restore.emit(doc);refresh.set(*refresh+1);error.set(String::new());},Err(e)=>error.set(e)}busy.set(false);});})};
    let historical=vulse_model::calculate_document(&revision.blueprint,&revision.body).0;
    html!{<details><summary>{format!("{} · {} · {}{}",title,revision.action,revision.created_at,if revision.source.as_deref()==Some("mcp"){" · via AI assistant (MCP)"}else{""})}</summary>
    if let Some(current)=&props.document {<table class="diff-table"><thead><tr><th>{"Field"}</th><th>{"This version"}</th><th>{"Current"}</th></tr></thead><tbody>{for props.collection.blueprint.fields.iter().chain(revision.blueprint.fields.iter().filter(|f|!props.collection.blueprint.fields.iter().any(|c|c.name==f.name))).filter(|f|historical.get(&f.name)!=current.body.get(&f.name)).map(|field|html!{<tr><th>{field.display_label()}</th><td>{vulse_model::display_value(&historical[&field.name])}</td><td>{vulse_model::display_value(&current.body[&field.name])}</td></tr>})}</tbody></table>}
    <pre>{serde_json::to_string_pretty(&historical).unwrap_or_default()}</pre><button type="button" class="secondary" disabled={*busy} onclick={restore}>{"Restore this version"}</button>
    if props.document.is_none(){<Link<Route> to={Route::Document{id:props.collection.id.clone(),doc_id:revision.document_id.clone()}}>{"Open restored record"}</Link<Route>>}
    </details>}})}
    <div class="row"><button class="secondary" disabled={*offset==0} onclick={Callback::from({let offset=offset.clone();move |_|offset.set(offset.saturating_sub(20))})}>{"Newer"}</button><button class="secondary" disabled={!*more} onclick={Callback::from({let offset=offset.clone();move |_|offset.set(*offset+20)})}>{"Older"}</button></div></section>}
}
