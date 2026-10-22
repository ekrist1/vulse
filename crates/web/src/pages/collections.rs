use vulse_model::{Blueprint, CollectionView, CreateCollection, Visibility};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::app::{Route, SessionCtx, Shell, signed_out};

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Name,
    Fields,
    Records,
    Updated,
    Access,
}

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
    // The column the list is sorted by, and whether it is descending.
    let sort = use_state(|| (Sort::Name, false));
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
    let is_admin = session.is_admin();
    let calculated = items
        .iter()
        .flat_map(|c| &c.blueprint.fields)
        .filter(|f| f.kind == vulse_model::FieldType::Calculated)
        .count();
    let public = items
        .iter()
        .filter(|c| c.visibility == Visibility::Public)
        .count();
    let summary = format!(
        "{} {} · {calculated} calculated {} · {public} public",
        items.len(),
        if items.len() == 1 {
            "collection"
        } else {
            "collections"
        },
        if calculated == 1 { "field" } else { "fields" }
    );
    let needle = search.trim().to_lowercase();
    let mut shown: Vec<&CollectionView> = items
        .iter()
        .filter(|c| c.name.to_lowercase().contains(&needle))
        .collect();
    shown.sort_by(|a, b| {
        let order = match sort.0 {
            Sort::Name => a.name.cmp(&b.name),
            Sort::Fields => a.blueprint.fields.len().cmp(&b.blueprint.fields.len()),
            Sort::Records => a.record_count.cmp(&b.record_count),
            Sort::Updated => a.updated_at.cmp(&b.updated_at),
            Sort::Access => {
                (a.visibility == Visibility::Public).cmp(&(b.visibility == Visibility::Public))
            }
        }
        .then_with(|| a.name.cmp(&b.name));
        if sort.1 { order.reverse() } else { order }
    });
    html! {
        <Shell>
            <div class="page-heading"><div><p class="eyebrow">{"SHARED WORKSPACE"}</p><h1>{"A home for your data."}</h1><p class="muted">{"Organize, calculate and create with collections."}</p></div>if is_admin {<button onclick={Callback::from({let show_create=show_create.clone();move |_|show_create.set(!*show_create)})}>{if *show_create {"Close"}else{"+ New collection"}}</button>}</div>
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
            <div class="section-heading"><div><h2>{"Collections"}</h2><p class="muted collections-stats">{summary}</p></div><input class="search" type="search" aria-label="Search collections" placeholder="Find a collection…" value={(*search).clone()} oninput={Callback::from({let search=search.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();search.set(input.value());}})}/></div>
            if !*loaded {<p class="muted" role="status">{"Loading collections…"}</p>}
            else if items.is_empty() && is_admin {<div class="card empty-state"><span class="empty-icon">{"▤"}</span><h2>{"Your first collection starts here"}</h2><p>{"Create a collection, define a few fields, and add or import your records."}</p><button onclick={Callback::from({let show_create=show_create.clone();move |_|show_create.set(true)})}>{"Create a collection"}</button></div>}
            else if items.is_empty() {<div class="card empty-state"><span class="empty-icon">{"▤"}</span><h2>{"No collections yet"}</h2><p>{"A workspace admin creates collections. They will appear here."}</p></div>}
            else if shown.is_empty() {<p class="muted">{"No collections match your search."}</p>}
            else {<div class="card data-card"><div class="table-scroll"><table class="collections-table">
                <thead><tr>{for [(Sort::Name, "Name", ""), (Sort::Fields, "Fields", "numeric optional"), (Sort::Records, "Records", "numeric"), (Sort::Updated, "Updated", ""), (Sort::Access, "Access", "optional")].into_iter().map(|(key, label, class)| {
                    let active = sort.0 == key;
                    let aria = active.then(|| if sort.1 { "descending" } else { "ascending" });
                    let onclick = {
                        let sort = sort.clone();
                        // Numbers and dates read best largest or newest first.
                        Callback::from(move |_| sort.set(if sort.0 == key { (key, !sort.1) } else { (key, matches!(key, Sort::Fields | Sort::Records | Sort::Updated)) }))
                    };
                    html! {<th class={class} aria-sort={aria}><button type="button" class="sort-button" {onclick}>{label}<span aria-hidden="true">{if !active {""} else if sort.1 {" ↓"} else {" ↑"}}</span></button></th>}
                })}</tr></thead>
                <tbody>{ for shown.iter().map(|collection| {
                    let id = collection.id.clone();
                    html! {
                        <tr key={id.clone()}>
                            <td><Link<Route> classes="row-link" to={Route::Collection { id }}>{collection.name.clone()}</Link<Route>></td>
                            <td class="numeric optional">{collection.blueprint.fields.len()}</td>
                            <td class="numeric">{collection.record_count.map(super::sql::group_digits).unwrap_or_default()}</td>
                            <td>{collection.updated_at.chars().take(10).collect::<String>()}</td>
                            <td class="optional">if collection.visibility == Visibility::Public {<span class="badge public">{"Public"}</span>} else {<span class="muted">{"Private"}</span>}</td>
                        </tr>
                    }
                }) }</tbody>
            </table></div></div>}
        </Shell>
    }
}
