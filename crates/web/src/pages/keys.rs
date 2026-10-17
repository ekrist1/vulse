use vulse_model::{
    ApiKeyView, CollectionView, CreateApiKey, OAuthClientView, OAuthGrantView, OAuthSettings, Role,
};
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
    // None while MCP is off on this server.
    let grants = use_state(|| None::<Vec<OAuthGrantView>>);
    let copied = use_state(|| false);
    let admin = session.role() == Role::Admin;
    {
        let keys = keys.clone();
        let collections = collections.clone();
        let grants = grants.clone();
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
                grants.set(api::oauth_grants(admin).await.ok());
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
            if let Some(items) = &*grants {
                <h2>{if admin {"Connected apps in the workspace"} else {"Connected apps"}}</h2>
                <p class="muted">
                    {"Apps such as Claude that were allowed to use Vulse through MCP. To connect one, give it this server address: "}
                    <code>{mcp_address()}</code>
                    {" "}
                    <button type="button" class="secondary" onclick={Callback::from({
                        let copied = copied.clone();
                        move |_| {
                            if let Some(window) = web_sys::window() {
                                let _ = window.navigator().clipboard().write_text(&mcp_address());
                                copied.set(true);
                            }
                        }
                    })}>{if *copied {"Copied"} else {"Copy"}}</button>
                </p>
                <p class="muted">
                    {"In Claude, open Settings → Connectors, add a custom connector with this address, and approve it here when asked."}
                </p>
                if items.is_empty() {
                    <p class="muted">{"No apps are connected."}</p>
                }
                <ul class="list">
                    { for items.iter().map(|grant| {
                        let id = grant.id.clone();
                        let grants = grants.clone();
                        let error = error.clone();
                        let confirm = confirm.clone();
                        let name = grant.client_name.clone();
                        let on_disconnect = Callback::from(move |_| {
                            let id = id.clone();
                            let grants = grants.clone();
                            let error = error.clone();
                            let confirm = confirm.clone();
                            let request = ConfirmRequest::danger(
                                format!("Disconnect {name}?"),
                                "The app loses access immediately. To use it again, connect it again and approve it.",
                                "Disconnect",
                            );
                            wasm_bindgen_futures::spawn_local(async move {
                                if !confirm.ask(request).await {
                                    return;
                                }
                                match api::delete_oauth_grant(&id).await {
                                    Ok(()) => grants.set((*grants).as_ref().map(|items| {
                                        items.iter().filter(|grant| grant.id != id).cloned().collect()
                                    })),
                                    Err(err) => error.set(err),
                                }
                            });
                        });
                        let access = if grant.write { "Read and change records" } else { "Read only" };
                        let scope = grant
                            .collection_id
                            .as_ref()
                            .map(|id| {
                                collections
                                    .iter()
                                    .find(|collection| collection.id == *id)
                                    .map_or_else(|| "One collection".to_string(), |collection| collection.name.clone())
                            })
                            .unwrap_or_else(|| "All collections".into());
                        let active = grant
                            .last_used_at
                            .as_deref()
                            .unwrap_or(&grant.created_at)
                            .get(..10)
                            .unwrap_or_default()
                            .to_string();
                        html! {
                            <li key={grant.id.clone()}>
                                <div>
                                    <strong>{grant.client_name.clone()}</strong>
                                    <span class="muted">
                                        {format!(" {access} · {scope} · active {active}")}
                                        {grant.handle.as_ref().map(|handle| format!(" · @{handle}")).unwrap_or_default()}
                                    </span>
                                </div>
                                <button type="button" class="danger" onclick={on_disconnect}>{"Disconnect"}</button>
                            </li>
                        }
                    }) }
                </ul>
                if admin {
                    <McpClients on_removed={Callback::from({
                        let grants = grants.clone();
                        move |_| {
                            let grants = grants.clone();
                            wasm_bindgen_futures::spawn_local(async move {
                                grants.set(api::oauth_grants(true).await.ok());
                            });
                        }
                    })} />
                }
            }
        </Shell>
    }
}

/// The MCP endpoint on this server, as apps should be given it.
fn mcp_address() -> String {
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();
    format!("{origin}/mcp")
}

#[derive(Properties, PartialEq)]
struct McpClientsProps {
    /// Called after removing an app, which disconnects its users.
    on_removed: Callback<()>,
}

/// For admins: whether any app may connect, and the apps Vulse knows.
#[function_component(McpClients)]
fn mcp_clients(props: &McpClientsProps) -> Html {
    let confirm = use_confirm();
    let settings = use_state(|| None::<OAuthSettings>);
    let clients = use_state(Vec::<OAuthClientView>::new);
    let error = use_state(String::new);
    let version = use_state(|| 0u32);
    {
        let settings = settings.clone();
        let clients = clients.clone();
        let error = error.clone();
        use_effect_with(*version, move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match api::oauth_settings().await {
                    Ok(found) => settings.set(Some(found)),
                    Err(err) => error.set(err),
                }
                match api::oauth_clients().await {
                    Ok(items) => clients.set(items),
                    Err(err) => error.set(err),
                }
            });
        });
    }
    let Some(current) = *settings else {
        return html! {};
    };
    let on_toggle = {
        let settings = settings.clone();
        let error = error.clone();
        Callback::from(move |event: Event| {
            let element: HtmlInputElement = event.target_unchecked_into();
            let body = OAuthSettings {
                open_registration: element.checked(),
            };
            let settings = settings.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::update_oauth_settings(&body).await {
                    Ok(saved) => settings.set(Some(saved)),
                    Err(err) => error.set(err),
                }
            });
        })
    };
    html! {
        <>
            <h2>{"Apps that can connect"}</h2>
            <label class="check">
                <input type="checkbox" checked={current.open_registration} onchange={on_toggle} />
                {"Let any app connect once a user approves it"}
            </label>
            <p class="muted">{"When this is off, a new app appears below and no one can connect it until you approve it. Apps you approve stay allowed. Apps marked “verified domain” are identified by their own web address, so one approval covers everyone using them."}</p>
            if !error.is_empty() {
                <p class="error" role="alert">{(*error).clone()}</p>
            }
            <ul class="list">
                { for clients.iter().map(|client| {
                    let approve = {
                        let id = client.id.clone();
                        let version = version.clone();
                        let error = error.clone();
                        Callback::from(move |_| {
                            let id = id.clone();
                            let version = version.clone();
                            let error = error.clone();
                            wasm_bindgen_futures::spawn_local(async move {
                                match api::approve_oauth_client(&id).await {
                                    Ok(()) => version.set(*version + 1),
                                    Err(err) => error.set(err),
                                }
                            });
                        })
                    };
                    let remove = {
                        let id = client.id.clone();
                        let name = client.name.clone();
                        let version = version.clone();
                        let error = error.clone();
                        let confirm = confirm.clone();
                        let on_removed = props.on_removed.clone();
                        Callback::from(move |_| {
                            let id = id.clone();
                            let version = version.clone();
                            let error = error.clone();
                            let confirm = confirm.clone();
                            let on_removed = on_removed.clone();
                            let request = ConfirmRequest::danger(
                                format!("Remove {name}?"),
                                "Everyone using this app is disconnected. It can ask to connect again later; while open connections are off, it then needs your approval.",
                                "Remove app",
                            );
                            wasm_bindgen_futures::spawn_local(async move {
                                if !confirm.ask(request).await {
                                    return;
                                }
                                match api::delete_oauth_client(&id).await {
                                    Ok(()) => {
                                        version.set(*version + 1);
                                        on_removed.emit(());
                                    }
                                    Err(err) => error.set(err),
                                }
                            });
                        })
                    };
                    let source = if client.metadata_document {
                        format!("verified domain {}", client.id.split('/').nth(2).unwrap_or_default())
                    } else {
                        format!("returns to {}", client.redirect_hosts.join(", "))
                    };
                    html! {
                        <li key={client.id.clone()}>
                            <div>
                                <strong>{client.name.clone()}</strong>
                                <span class="muted">
                                    {format!(" {source} · {} connected · added {}", client.connections, client.created_at.get(..10).unwrap_or_default())}
                                    {if client.approved { "" } else { " · waiting for approval" }}
                                </span>
                            </div>
                            <div class="row">
                                if !client.approved {
                                    <button type="button" onclick={approve}>{"Approve"}</button>
                                }
                                <button type="button" class="danger" onclick={remove}>{"Remove"}</button>
                            </div>
                        </li>
                    }
                }) }
            </ul>
        </>
    }
}
