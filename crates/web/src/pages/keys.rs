use vulse_model::{ApiKeyView, CollectionView, CreateApiKey};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

use crate::api;
use crate::app::{SessionCtx, Shell, signed_out};

#[function_component(KeysPage)]
pub fn keys_page() -> Html {
    let session = use_context::<SessionCtx>().expect("session");
    let keys = use_state(Vec::<ApiKeyView>::new);
    let collections = use_state(Vec::<CollectionView>::new);
    let name = use_state(|| "website".to_string());
    let collection_id = use_state(String::new);
    let secret = use_state(String::new);
    let error = use_state(String::new);
    {
        let keys = keys.clone();
        let collections = collections.clone();
        let error = error.clone();
        let authed = session.state.user.is_some();
        use_effect_with(authed, move |authed| {
            if !*authed {
                return;
            }
            let keys = keys.clone();
            let collections = collections.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::keys().await {
                    Ok(items) => keys.set(items),
                    Err(err) => error.set(err),
                }
                if let Ok(items) = api::collections().await {
                    collections.set(items);
                }
            });
        });
    }
    let onsubmit = {
        let name = name.clone();
        let collection_id = collection_id.clone();
        let keys = keys.clone();
        let secret = secret.clone();
        let error = error.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let body = CreateApiKey {
                name: (*name).clone(),
                collection_id: if collection_id.is_empty() {
                    None
                } else {
                    Some((*collection_id).clone())
                },
            };
            let keys = keys.clone();
            let secret = secret.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::create_key(&body).await {
                    Ok(created) => {
                        let mut next = (*keys).clone();
                        next.insert(0, created.key);
                        keys.set(next);
                        secret.set(created.secret);
                        error.set(String::new());
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
            <h1>{"API keys"}</h1>
            <p class="muted">{"A key can reach every collection you own, or just one. The secret is shown once."}</p>
            if !secret.is_empty() {
                <p class="secret" role="status">
                    {"Copy this secret now: "}
                    <code>{(*secret).clone()}</code>
                </p>
            }
            <form class="card" onsubmit={onsubmit}>
                <label>{"Name"}
                    <input required=true value={(*name).clone()} oninput={Callback::from({
                        let name = name.clone();
                        move |event: InputEvent| {
                            let element: HtmlInputElement = event.target_unchecked_into();
                            name.set(element.value());
                        }
                    })} />
                </label>
                <label>{"Scope"}
                    <select onchange={Callback::from({
                        let collection_id = collection_id.clone();
                        move |event: Event| {
                            let element: HtmlSelectElement = event.target_unchecked_into();
                            collection_id.set(element.value());
                        }
                    })}>
                        <option value="" selected={collection_id.is_empty()}>{"All collections"}</option>
                        { for collections.iter().map(|collection| html! {
                            <option value={collection.id.clone()} selected={*collection_id == collection.id}>{collection.name.clone()}</option>
                        }) }
                    </select>
                </label>
                if !error.is_empty() {
                    <p class="error" role="alert">{(*error).clone()}</p>
                }
                <button type="submit">{"Create key"}</button>
            </form>
            <ul class="list">
                { for keys.iter().map(|key| {
                    let id = key.id.clone();
                    let keys = keys.clone();
                    let error = error.clone();
                    let on_delete = Callback::from(move |_| {
                        let id = id.clone();
                        let keys = keys.clone();
                        let error = error.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            match api::delete_key(&id).await {
                                Ok(()) => keys.set((*keys).iter().filter(|key| key.id != id).cloned().collect()),
                                Err(err) => error.set(err),
                            }
                        });
                    });
                    html! {
                        <li key={key.id.clone()}>
                            <div>
                                <strong>{key.name.clone()}</strong>
                                <span class="muted">{format!(" {}", key.prefix)}</span>
                            </div>
                            <button type="button" class="danger" onclick={on_delete}>{"Revoke"}</button>
                        </li>
                    }
                }) }
            </ul>
        </Shell>
    }
}
