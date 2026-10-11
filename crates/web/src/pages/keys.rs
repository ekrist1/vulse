use vulse_model::{ApiKeyView, CollectionView, CreateApiKey, Role};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

use crate::api;
use crate::app::{SessionCtx, Shell, signed_out};
use crate::confirm::{ConfirmRequest, use_confirm};

#[function_component(KeysPage)]
pub fn keys_page() -> Html {
    let confirm = use_confirm();
    let session = use_context::<SessionCtx>().expect("session");
    let keys = use_state(Vec::<ApiKeyView>::new);
    let collections = use_state(Vec::<CollectionView>::new);
    let name = use_state(|| "website".to_string());
    let collection_id = use_state(String::new);
    let role = use_state(|| None::<Role>);
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
        let role = role.clone();
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
                role: *role,
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
    let my_role = session.role();
    html! {
        <Shell>
            <h1>{"API keys"}</h1>
            <p class="muted">{"A key can reach every collection in the workspace, or just one. Its role can be lower than yours, never higher, and drops if your role is lowered. Disabling your account stops all your keys. The secret is shown once."}</p>
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
                <label>{"Role"}
                    <select onchange={Callback::from({
                        let role = role.clone();
                        move |event: Event| {
                            let element: HtmlSelectElement = event.target_unchecked_into();
                            role.set(Role::parse(&element.value()));
                        }
                    })}>
                        { for Role::ALL.into_iter().filter(|option| *option <= my_role).rev().map(|option| html! {
                            <option value={option.as_str()} selected={role.unwrap_or(my_role) == option}>
                                {match option {
                                    Role::Viewer => "Viewer — read only",
                                    Role::Editor => "Editor — read and write records and files",
                                    Role::Admin => "Admin — also change collection schemas",
                                }}
                            </option>
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
                    let confirm = confirm.clone();
                    let name = key.name.clone();
                    let on_delete = Callback::from(move |_| {
                        let id = id.clone();
                        let keys = keys.clone();
                        let error = error.clone();
                        let confirm = confirm.clone();
                        let request = ConfirmRequest::danger(
                            format!("Revoke the key “{name}”?"),
                            "Anything using this key stops working immediately. A revoked key cannot be restored; you would need to create a new one.",
                            "Revoke key",
                        );
                        wasm_bindgen_futures::spawn_local(async move {
                            if !confirm.ask(request).await {
                                return;
                            }
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
                                <span class="muted">{format!(" {} · {}", key.prefix, key.role.label())}</span>
                            </div>
                            <button type="button" class="danger" onclick={on_delete}>{"Revoke"}</button>
                        </li>
                    }
                }) }
            </ul>
        </Shell>
    }
}
