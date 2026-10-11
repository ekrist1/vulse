use vulse_model::{
    CreateInvite, CreatedInvite, InviteView, LinkPurpose, ManagedUser, Role, UpdateUser,
};
use web_sys::HtmlSelectElement;
use yew::prelude::*;

use crate::api;
use crate::app::{SessionCtx, Shell, signed_out};
use crate::confirm::{ConfirmRequest, use_confirm};

/// Admin-only: workspace members, roles, invites and reset links.
#[function_component(PeoplePage)]
pub fn people_page() -> Html {
    let confirm = use_confirm();
    let session = use_context::<SessionCtx>().expect("session");
    let users = use_state(Vec::<ManagedUser>::new);
    let invites = use_state(Vec::<InviteView>::new);
    let email = use_state(String::new);
    let role = use_state(|| Role::Editor);
    let link = use_state(|| None::<CreatedInvite>);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let refresh = use_state(|| 0u32);
    let is_admin = session
        .state
        .user
        .as_ref()
        .is_some_and(|u| u.role == Role::Admin);
    {
        let users = users.clone();
        let invites = invites.clone();
        let error = error.clone();
        use_effect_with((is_admin, *refresh), move |(is_admin, _)| {
            if !*is_admin {
                return;
            }
            wasm_bindgen_futures::spawn_local(async move {
                match api::users().await {
                    Ok(items) => users.set(items),
                    Err(err) => error.set(err),
                }
                match api::invites().await {
                    Ok(items) => invites.set(items),
                    Err(err) => error.set(err),
                }
            });
        });
    }
    let reload = {
        let refresh = refresh.clone();
        Callback::from(move |_: ()| refresh.set(*refresh + 1))
    };
    let on_invite = {
        let email = email.clone();
        let role = role.clone();
        let link = link.clone();
        let error = error.clone();
        let busy = busy.clone();
        let reload = reload.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let body = CreateInvite {
                email: (*email).clone(),
                role: *role,
            };
            let email = email.clone();
            let link = link.clone();
            let error = error.clone();
            let busy = busy.clone();
            let reload = reload.clone();
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::create_invite(&body).await {
                    Ok(created) => {
                        link.set(Some(created));
                        email.set(String::new());
                        error.set(String::new());
                        reload.emit(());
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
    if !is_admin {
        return html! {
            <Shell>
                <h1>{"People"}</h1>
                <p class="error" role="alert">{"Only workspace admins can manage people."}</p>
            </Shell>
        };
    }
    let me = session
        .state
        .user
        .as_ref()
        .map(|u| u.id.clone())
        .unwrap_or_default();
    let update = {
        let error = error.clone();
        let reload = reload.clone();
        Callback::from(move |(id, body): (String, UpdateUser)| {
            let error = error.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::update_user(&id, &body).await {
                    Ok(_) => error.set(String::new()),
                    Err(err) => error.set(err),
                }
                reload.emit(());
            });
        })
    };
    let reset = {
        let link = link.clone();
        let error = error.clone();
        let reload = reload.clone();
        Callback::from(move |id: String| {
            let link = link.clone();
            let error = error.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::reset_password(&id).await {
                    Ok(created) => {
                        link.set(Some(created));
                        error.set(String::new());
                    }
                    Err(err) => error.set(err),
                }
                reload.emit(());
            });
        })
    };
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    html! {
        <Shell>
            <div class="page-heading"><div>
                <p class="eyebrow">{"WORKSPACE"}</p>
                <h1>{"People"}</h1>
                <p class="muted">{"Viewers read everything. Editors also change records, files, imports and saved views. Admins also change collection schemas and manage people."}</p>
            </div></div>
            if let Some(created) = (*link).clone() {
                <section class="secret" role="status">
                    <p><strong>{match created.invite.purpose {
                        LinkPurpose::Invite => format!("Invite link for {}", created.invite.email),
                        LinkPurpose::Reset => format!("Password reset link for {}", created.invite.email),
                    }}</strong></p>
                    <p>{format!("Send this link to them privately. It works once and expires {}. It is shown only now.", short_time(&created.invite.expires_at))}</p>
                    <code>{format!("{origin}{}", created.path)}</code>
                </section>
            }
            {crate::pages::auth::error_line(&error)}
            <form class="card create-card" onsubmit={on_invite}>
                <h2>{"Invite someone"}</h2>
                <label>{"Email"}<input type="email" required=true value={(*email).clone()} oninput={crate::pages::auth::text_input(email.clone())} /></label>
                <label>{"Role"}
                    <select onchange={Callback::from({
                        let role = role.clone();
                        move |event: Event| {
                            let element: HtmlSelectElement = event.target_unchecked_into();
                            role.set(Role::parse(&element.value()).unwrap_or(Role::Viewer));
                        }
                    })}>
                        { for Role::ALL.into_iter().map(|option| html! {
                            <option value={option.as_str()} selected={*role == option}>{option.label()}</option>
                        }) }
                    </select>
                </label>
                <button type="submit" disabled={*busy}>{if *busy {"Creating…"} else {"Create invite link"}}</button>
            </form>
            <section class="card">
                <h2>{"Members"}</h2>
                <div class="table-scroll"><table>
                    <thead><tr><th>{"Handle"}</th><th>{"Email"}</th><th>{"Role"}</th><th>{"Status"}</th><th>{"Actions"}</th></tr></thead>
                    <tbody>
                        { for users.iter().map(|user| {
                            let id = user.id.clone();
                            let disabled = user.disabled_at.is_some();
                            let on_role = {
                                let update = update.clone();
                                let id = id.clone();
                                Callback::from(move |event: Event| {
                                    let element: HtmlSelectElement = event.target_unchecked_into();
                                    if let Some(role) = Role::parse(&element.value()) {
                                        update.emit((id.clone(), UpdateUser { role: Some(role), disabled: None }));
                                    }
                                })
                            };
                            let on_toggle = {
                                let update = update.clone();
                                let id = id.clone();
                                let email = user.email.clone();
                                let confirm = confirm.clone();
                                Callback::from(move |_| {
                                    let change = (id.clone(), UpdateUser { role: None, disabled: Some(!disabled) });
                                    if disabled {
                                        update.emit(change);
                                        return;
                                    }
                                    let (update, confirm) = (update.clone(), confirm.clone());
                                    let request = ConfirmRequest::danger(
                                        format!("Disable {email}?"),
                                        "Their sessions end and their API keys stop working at once. You can enable the account again later.",
                                        "Disable account",
                                    );
                                    wasm_bindgen_futures::spawn_local(async move {
                                        if confirm.ask(request).await {
                                            update.emit(change);
                                        }
                                    });
                                })
                            };
                            let on_reset = {
                                let reset = reset.clone();
                                let id = id.clone();
                                Callback::from(move |_| reset.emit(id.clone()))
                            };
                            html! {
                                <tr key={id.clone()}>
                                    <td>{format!("@{}", user.handle)}{if id == me {" (you)"} else {""}}</td>
                                    <td>{user.email.clone()}</td>
                                    <td>
                                        <select aria-label={format!("Role for {}", user.email)} onchange={on_role}>
                                            { for Role::ALL.into_iter().map(|option| html! {
                                                <option value={option.as_str()} selected={user.role == option}>{option.label()}</option>
                                            }) }
                                        </select>
                                    </td>
                                    <td>{if disabled {"Disabled"} else {"Active"}}</td>
                                    <td><div class="row">
                                        <button type="button" class="secondary" disabled={disabled} onclick={on_reset}>{"Reset link"}</button>
                                        <button type="button" class={if disabled {"secondary"} else {"danger"}} onclick={on_toggle}>{if disabled {"Enable"} else {"Disable"}}</button>
                                    </div></td>
                                </tr>
                            }
                        }) }
                    </tbody>
                </table></div>
            </section>
            if !invites.is_empty() {
                <section class="card">
                    <h2>{"Pending links"}</h2>
                    <ul class="list">
                        { for invites.iter().map(|invite| {
                            let id = invite.id.clone();
                            let error = error.clone();
                            let reload = reload.clone();
                            let confirm = confirm.clone();
                            let email = invite.email.clone();
                            let on_revoke = Callback::from(move |_| {
                                let id = id.clone();
                                let error = error.clone();
                                let reload = reload.clone();
                                let confirm = confirm.clone();
                                let request = ConfirmRequest::danger(
                                    format!("Revoke the link for {email}?"),
                                    "The link stops working immediately. You can create a new one later.",
                                    "Revoke link",
                                );
                                wasm_bindgen_futures::spawn_local(async move {
                                    if !confirm.ask(request).await {
                                        return;
                                    }
                                    if let Err(err) = api::delete_invite(&id).await {
                                        error.set(err);
                                    }
                                    reload.emit(());
                                });
                            });
                            html! {
                                <li key={invite.id.clone()}>
                                    <div>
                                        <strong>{invite.email.clone()}</strong>
                                        <span class="muted">{format!(" · {} · {} · expires {}",
                                            if invite.purpose == LinkPurpose::Invite {"invite"} else {"password reset"},
                                            invite.role.label(),
                                            short_time(&invite.expires_at))}</span>
                                    </div>
                                    <button type="button" class="danger" onclick={on_revoke}>{"Revoke"}</button>
                                </li>
                            }
                        }) }
                    </ul>
                </section>
            }
        </Shell>
    }
}

fn short_time(value: &str) -> String {
    value.chars().take(16).collect::<String>().replace('T', " ") + " UTC"
}
