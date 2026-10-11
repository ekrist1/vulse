use crate::{api, app::Route};
use vulse_model::{CollectionView, DocumentView, ReferenceView};
use yew::prelude::*;
use yew_router::prelude::*;
#[derive(Properties, PartialEq)]
pub struct LinksProps {
    pub collection: Option<CollectionView>,
    pub ids: Vec<String>,
}
#[function_component(ReferenceLinks)]
pub fn reference_links(props: &LinksProps) -> Html {
    let documents = use_state(Vec::<DocumentView>::new);
    let error = use_state(String::new);
    {
        let documents = documents.clone();
        let error = error.clone();
        use_effect_with(
            (props.collection.clone(), props.ids.clone()),
            move |(collection, ids)| {
                let (collection, ids) = (collection.clone(), ids.clone());
                documents.set(vec![]);
                error.set(String::new());
                wasm_bindgen_futures::spawn_local(async move {
                    if let Some(c) = collection {
                        let mut next = vec![];
                        for id in ids {
                            match api::document(&c.id, &id).await {
                                Ok(d) => next.push(d),
                                Err(e) => error.set(e),
                            }
                        }
                        documents.set(next);
                    } else if !ids.is_empty() {
                        error.set("Referenced collection unavailable".into());
                    }
                });
            },
        );
    }
    html! {<div class="reference-links">{for documents.iter().map(|d|{let c=props.collection.as_ref().unwrap();html!{<Link<Route> to={Route::Document{id:c.id.clone(),doc_id:d.id.clone()}}>{format!("{} →",vulse_model::record_title(&c.blueprint,&d.body,&d.id))}</Link<Route>>}})}if !error.is_empty(){<span class="error">{(*error).clone()}</span>}</div>}
}
#[derive(Properties, PartialEq)]
pub struct BacklinksProps {
    pub collection_id: String,
    pub document_id: String,
    pub version: String,
}
#[function_component(Backlinks)]
pub fn backlinks(props: &BacklinksProps) -> Html {
    let links = use_state(Vec::<ReferenceView>::new);
    let offset = use_state(|| 0u32);
    let more = use_state(|| false);
    let error = use_state(String::new);
    {
        let links = links.clone();
        let error = error.clone();
        let more = more.clone();
        use_effect_with(
            (
                props.collection_id.clone(),
                props.document_id.clone(),
                props.version.clone(),
                *offset,
            ),
            move |(id, doc, _, offset)| {
                let (id, doc, offset) = (id.clone(), doc.clone(), *offset);
                wasm_bindgen_futures::spawn_local(async move {
                    match api::backlinks(&id, &doc, offset).await {
                        Ok(page) => {
                            more.set(page.items.len() == 25);
                            links.set(page.items);
                            error.set(String::new());
                        }
                        Err(e) => error.set(e),
                    }
                });
            },
        );
    }
    html! {<section class="card backlinks"><h2>{"Referenced by"}</h2><p class="muted">{"Records that link to this record."}</p>if !error.is_empty(){<p class="error" role="alert">{(*error).clone()}</p>}if links.is_empty(){<p class="muted">{"No incoming references."}</p>}<ul>{for links.iter().map(|r|html!{<li><Link<Route> to={Route::Document{id:r.collection_id.clone(),doc_id:r.document_id.clone()}}>{r.title.clone()}</Link<Route>>{format!(" · {} / {}",r.collection_name,r.field)}</li>})}</ul><div class="row"><button class="secondary" disabled={*offset==0} onclick={Callback::from({let offset=offset.clone();move |_|offset.set(offset.saturating_sub(25))})}>{"Previous links"}</button><button class="secondary" disabled={!*more} onclick={Callback::from({let offset=offset.clone();move |_|offset.set(*offset+25)})}>{"More links"}</button></div></section>}
}
