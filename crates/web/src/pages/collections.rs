use vulse_model::{Blueprint, CollectionView, CreateCollection, Visibility};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::app::{Route, SessionCtx, Shell, signed_out};

#[function_component(CollectionsPage)]
pub fn collections_page() -> Html {
    let session = use_context::<SessionCtx>().expect("session");
    let items = use_state(Vec::<CollectionView>::new);
    let error = use_state(String::new);
    let name = use_state(String::new);
    let show_create = use_state(|| false);
    let busy = use_state(|| false);
    let loaded = use_state(|| false);
    let search = use_state(String::new);
    let navigator = use_navigator();
    let visibility = use_state(|| Visibility::Private);
    {
        let items = items.clone();
        let error = error.clone();
        let loaded = loaded.clone();
        let authed = session.state.user.is_some();
        use_effect_with(authed, move |authed| {
            if !*authed {
                return;
            }
            wasm_bindgen_futures::spawn_local(async move {
                match api::collections().await {
                    Ok(collections) => items.set(collections),
                    Err(err) => error.set(err),
                }
                loaded.set(true);
            });
        });
    }
    let on_name = {
        let name = name.clone();
        Callback::from(move |event: InputEvent| {
            let element: HtmlInputElement = event.target_unchecked_into();
            name.set(element.value().to_ascii_lowercase());
        })
    };
    let on_visibility = {
        let visibility = visibility.clone();
        Callback::from(move |event: Event| {
            let element: HtmlSelectElement = event.target_unchecked_into();
            visibility.set(if element.value() == "public" {
                Visibility::Public
            } else {
                Visibility::Private
            });
        })
    };
    let onsubmit = {
        let name = name.clone();
        let visibility = visibility.clone();
        let items = items.clone();
        let error = error.clone();
        let busy = busy.clone();
        let navigator = navigator.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            if *busy {
                return;
            }
            busy.set(true);
            let body = CreateCollection {
                name: (*name).clone(),
                visibility: *visibility,
                blueprint: Blueprint {
                    title_field: None,
                    fields: Vec::new(),
                },
            };
            let items = items.clone();
            let error = error.clone();
            let busy = busy.clone();
            let navigator = navigator.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::create_collection(&body).await {
                    Ok(collection) => {
                        let mut next = (*items).clone();
                        let id = collection.id.clone();
                        next.insert(0, collection);
                        items.set(next);
                        error.set(String::new());
                        if let Some(navigator) = navigator {
                            navigator.push(&Route::Collection { id });
                        }
                    }
                    Err(err) => error.set(err),
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
            <div class="page-heading"><div><p class="eyebrow">{"YOUR WORKSPACE"}</p><h1>{"A home for your data."}</h1><p class="muted">{"Organize, calculate and create with collections."}</p></div><button onclick={Callback::from({let show_create=show_create.clone();move |_|show_create.set(!*show_create)})}>{if *show_create {"Close"}else{"+ New collection"}}</button></div>
            <div class="workspace-summary"><div><span class="summary-icon">{"▤"}</span><strong>{items.len()}</strong><span>{"Collections"}</span></div><div><span class="summary-icon">{"ƒx"}</span><strong>{items.iter().flat_map(|c|&c.blueprint.fields).filter(|f|f.kind==vulse_model::FieldType::Calculated).count()}</strong><span>{"Calculated fields"}</span></div><div><span class="summary-icon">{"↗"}</span><strong>{items.iter().filter(|c|c.visibility==Visibility::Public).count()}</strong><span>{"Public collections"}</span></div></div>
            if *show_create {<form class="card create-card" onsubmit={onsubmit}>
                <h2>{"New collection"}</h2>
                <label>{"Name"}<input placeholder="e.g. projects or inventory" required=true value={(*name).clone()} oninput={on_name} /></label>
                <label>{"Visibility"}
                    <select onchange={on_visibility}>
                        <option value="private" selected={*visibility == Visibility::Private}>{"Private"}</option>
                        <option value="public" selected={*visibility == Visibility::Public}>{"Public"}</option>
                    </select>
                </label>
                if !error.is_empty() {
                    <p class="error" role="alert">{(*error).clone()}</p>
                }
                <p class="muted">{"Use lowercase letters, numbers, hyphens or underscores. Add your fields after creating the collection."}</p><button type="submit" disabled={*busy}>{if *busy{"Creating…"}else{"Create collection →"}}</button>
            </form>}
            if !error.is_empty() && !*show_create {<p class="error" role="alert">{(*error).clone()}</p>}
            <div class="section-heading"><h2>{"Your collections"}</h2><input class="search" type="search" aria-label="Search collections" placeholder="Find a collection…" value={(*search).clone()} oninput={Callback::from({let search=search.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();search.set(input.value());}})}/></div>
            if !*loaded {<p class="muted" role="status">{"Loading collections…"}</p>}
            else if items.is_empty() {<div class="card empty-state"><span class="empty-icon">{"▤"}</span><h2>{"Your first collection starts here"}</h2><p>{"Create a collection, define a few fields, and add or import your records."}</p><button onclick={Callback::from({let show_create=show_create.clone();move |_|show_create.set(true)})}>{"Create a collection"}</button></div>}
            else if !items.iter().any(|c|c.name.contains(search.as_str())) {<p class="muted">{"No collections match your search."}</p>}
            <ul class="collection-grid">
                { for items.iter().filter(|c|c.name.contains(search.as_str())).map(|collection| {
                    let id = collection.id.clone();
                    html! {
                        <li key={id.clone()}>
                            <Link<Route> classes="collection-link" to={Route::Collection { id }}>
                                <span class="collection-icon">{"▤"}</span><h3>{collection.name.clone()}</h3><p class="muted">{format!("{} fields · Updated {}",collection.blueprint.fields.len(),collection.updated_at.chars().take(10).collect::<String>())}</p><span class="collection-open">{"Open collection →"}</span>
                            </Link<Route>>
                            <span class={if collection.visibility == Visibility::Public { "badge public" } else { "badge" }}>
                                {if collection.visibility == Visibility::Public { "Public" } else { "Private" }}
                            </span>
                        </li>
                    }
                }) }
            </ul>
        </Shell>
    }
}
