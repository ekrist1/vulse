use vulse_model::{AcceptLink, LinkInfo, LinkPurpose, LoginRequest, SetupRequest, UserView};
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::app::{Route, SessionCtx, SessionState, Shell};

#[function_component(LoginPage)]
pub fn login_page() -> Html {
    let email = use_state(String::new);
    let password = use_state(String::new);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let session = use_context::<SessionCtx>().expect("session");
    let navigator = use_navigator().unwrap();
    {
        // A fresh install has no accounts; send the operator to setup.
        let navigator = navigator.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                if api::setup_status()
                    .await
                    .is_ok_and(|status| status.needs_setup)
                {
                    navigator.replace(&Route::Setup);
                }
            });
        });
    }
    if session.state.user.is_some() {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }
    let onsubmit = {
        let email = email.clone();
        let password = password.clone();
        let error = error.clone();
        let busy = busy.clone();
        let session = session.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let body = LoginRequest {
                email: (*email).clone(),
                password: (*password).clone(),
            };
            let error = error.clone();
            let busy = busy.clone();
            let session = session.clone();
            let navigator = navigator.clone();
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::login(&body).await {
                    Ok(user) => signed_in(&session, &navigator, user),
                    Err(err) => error.set(sign_in_error(err)),
                }
                busy.set(false);
            });
        })
    };
    html! {
        <Shell>
            <h1>{"Log in"}</h1>
            <form onsubmit={onsubmit}>
                <label>{"Email"}<input type="email" autocomplete="username" required=true value={(*email).clone()} oninput={text_input(email)} /></label>
                <label>{"Password"}<input type="password" autocomplete="current-password" required=true value={(*password).clone()} oninput={text_input(password)} /></label>
                {error_line(&error)}
                <button type="submit" disabled={*busy}>{if *busy {"Logging in…"} else {"Log in"}}</button>
            </form>
            <p class="muted">{"Accounts are created by a workspace admin. Ask an admin for an invite or a password reset link."}</p>
        </Shell>
    }
}

#[function_component(SetupPage)]
pub fn setup_page() -> Html {
    let token = use_state(String::new);
    let email = use_state(String::new);
    let handle = use_state(String::new);
    let password = use_state(String::new);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let session = use_context::<SessionCtx>().expect("session");
    let navigator = use_navigator().unwrap();
    let onsubmit = {
        let token = token.clone();
        let email = email.clone();
        let handle = handle.clone();
        let password = password.clone();
        let error = error.clone();
        let busy = busy.clone();
        let session = session.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let body = SetupRequest {
                token: (*token).clone(),
                email: (*email).clone(),
                handle: (*handle).clone(),
                password: (*password).clone(),
            };
            let error = error.clone();
            let busy = busy.clone();
            let session = session.clone();
            let navigator = navigator.clone();
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::setup(&body).await {
                    Ok(user) => signed_in(&session, &navigator, user),
                    Err(err) => error.set(err),
                }
                busy.set(false);
            });
        })
    };
    html! {
        <Shell>
            <p class="eyebrow">{"FIRST RUN"}</p>
            <h1>{"Set up your workspace"}</h1>
            <p class="muted">{"Create the first admin account. The setup token is printed in the server log when the server starts with no accounts, or set with VULSE_SETUP_TOKEN."}</p>
            <form class="card create-card" onsubmit={onsubmit}>
                <label>{"Setup token"}<input autocomplete="off" required=true value={(*token).clone()} oninput={text_input(token)} /></label>
                <label>{"Email"}<input type="email" autocomplete="username" required=true value={(*email).clone()} oninput={text_input(email)} /></label>
                <label>{"Handle"}<input required=true value={(*handle).clone()} oninput={text_input(handle)} /></label>
                <label>{"Password"}<input type="password" autocomplete="new-password" required=true minlength="8" value={(*password).clone()} oninput={text_input(password)} /></label>
                {error_line(&error)}
                <button type="submit" disabled={*busy}>{if *busy {"Creating…"} else {"Create admin account"}}</button>
            </form>
        </Shell>
    }
}

/// Completes an invite or password reset. The token arrives in the URL
/// fragment and is removed from the address bar once read.
#[function_component(AcceptPage)]
pub fn accept_page() -> Html {
    let token = use_state(take_fragment_token);
    let info = use_state(|| None::<LinkInfo>);
    let handle = use_state(String::new);
    let password = use_state(String::new);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let session = use_context::<SessionCtx>().expect("session");
    let navigator = use_navigator().unwrap();
    {
        let token = (*token).clone();
        let info = info.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                if token.is_empty() {
                    error.set("This link is missing its token. Copy the full link again.".into());
                    return;
                }
                match api::inspect_link(&token).await {
                    Ok(found) => info.set(Some(found)),
                    Err(err) => error.set(err),
                }
            });
        });
    }
    let onsubmit = {
        let token = token.clone();
        let info = info.clone();
        let handle = handle.clone();
        let password = password.clone();
        let error = error.clone();
        let busy = busy.clone();
        let session = session.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let invite = info
                .as_ref()
                .is_some_and(|i| i.purpose == LinkPurpose::Invite);
            let body = AcceptLink {
                token: (*token).clone(),
                handle: invite.then(|| (*handle).clone()),
                password: (*password).clone(),
            };
            let error = error.clone();
            let busy = busy.clone();
            let session = session.clone();
            let navigator = navigator.clone();
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::accept_link(&body).await {
                    Ok(user) => signed_in(&session, &navigator, user),
                    Err(err) => error.set(err),
                }
                busy.set(false);
            });
        })
    };
    let Some(link) = (*info).clone() else {
        return html! {
            <Shell>
                <h1>{"Account link"}</h1>
                if error.is_empty() {
                    <p class="muted" role="status">{"Checking your link…"}</p>
                } else {
                    {error_line(&error)}
                    <p class="muted">{"Ask a workspace admin for a new link."}</p>
                }
            </Shell>
        };
    };
    let invite = link.purpose == LinkPurpose::Invite;
    html! {
        <Shell>
            <h1>{if invite {"Join the workspace"} else {"Choose a new password"}}</h1>
            <p class="muted">{if invite {
                format!("You were invited as {}. Choose a handle and a password to finish.", link.email)
            } else {
                format!("Set a new password for {}. Other sessions for this account will be signed out.", link.email)
            }}</p>
            <form class="card create-card" onsubmit={onsubmit}>
                <input type="email" autocomplete="username" value={link.email.clone()} hidden=true readonly=true />
                if invite {
                    <label>{"Handle"}<input required=true value={(*handle).clone()} oninput={text_input(handle)} /></label>
                    <p class="muted">{"2–32 lowercase letters, numbers or hyphens. Shown to other workspace members."}</p>
                }
                <label>{"Password"}<input type="password" autocomplete="new-password" required=true minlength="8" value={(*password).clone()} oninput={text_input(password)} /></label>
                {error_line(&error)}
                <button type="submit" disabled={*busy}>{if invite {"Create account"} else {"Save password"}}</button>
            </form>
        </Shell>
    }
}

fn take_fragment_token() -> String {
    let Some(window) = web_sys::window() else {
        return String::new();
    };
    let hash = window.location().hash().unwrap_or_default();
    let token = hash
        .trim_start_matches('#')
        .split('&')
        .find_map(|part| part.strip_prefix("token="))
        .unwrap_or("")
        .to_string();
    if !token.is_empty() {
        if let Ok(history) = window.history() {
            let _ =
                history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some("/accept"));
        }
    }
    token
}

fn signed_in(session: &SessionCtx, navigator: &Navigator, user: UserView) {
    session.set.emit(SessionState {
        user: Some(user),
        loaded: true,
    });
    navigator.push(&Route::Home);
}

fn sign_in_error(err: String) -> String {
    if err == "unauthorized" {
        "The email or password is not correct.".into()
    } else {
        err
    }
}

pub(crate) fn text_input(state: UseStateHandle<String>) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| {
        let element: HtmlInputElement = event.target_unchecked_into();
        state.set(element.value());
    })
}

pub(crate) fn error_line(error: &UseStateHandle<String>) -> Html {
    if error.is_empty() {
        html! {}
    } else {
        html! { <p class="error" role="alert">{(**error).clone()}</p> }
    }
}
