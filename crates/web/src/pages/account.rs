use vulse_model::ChangePassword;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::app::{Route, SessionCtx, SessionState, Shell, signed_out};
use crate::pages::auth::{error_line, text_input};

#[function_component(AccountPage)]
pub fn account_page() -> Html {
    let session = use_context::<SessionCtx>().expect("session");
    let navigator = use_navigator().unwrap();
    let current = use_state(String::new);
    let next = use_state(String::new);
    let error = use_state(String::new);
    let status = use_state(String::new);
    let busy = use_state(|| false);
    let on_change = {
        let current = current.clone();
        let next = next.clone();
        let error = error.clone();
        let status = status.clone();
        let busy = busy.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let body = ChangePassword {
                current_password: (*current).clone(),
                new_password: (*next).clone(),
            };
            let current = current.clone();
            let next = next.clone();
            let error = error.clone();
            let status = status.clone();
            let busy = busy.clone();
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::change_password(&body).await {
                    Ok(()) => {
                        current.set(String::new());
                        next.set(String::new());
                        error.set(String::new());
                        status.set("Password changed. Your other sessions were signed out.".into());
                    }
                    Err(err) => {
                        status.set(String::new());
                        error.set(err);
                    }
                }
                busy.set(false);
            });
        })
    };
    let on_everywhere = {
        let session = session.clone();
        let error = error.clone();
        Callback::from(move |_| {
            if !crate::app::confirm("Sign out of every browser and device, including this one?") {
                return;
            }
            let session = session.clone();
            let error = error.clone();
            let navigator = navigator.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::logout_everywhere().await {
                    Ok(()) => {
                        session.set.emit(SessionState {
                            user: None,
                            loaded: true,
                        });
                        navigator.push(&Route::Login);
                    }
                    Err(err) => error.set(err),
                }
            });
        })
    };
    if let Some(redirect) = signed_out(&session) {
        return redirect;
    }
    let user = session.state.user.clone().unwrap();
    html! {
        <Shell>
            <div class="page-heading"><div>
                <p class="eyebrow">{"ACCOUNT"}</p>
                <h1>{format!("@{}", user.handle)}</h1>
                <p class="muted">{format!("{} · {}", user.email, user.role.label())}</p>
            </div></div>
            if !status.is_empty() {
                <p class="success" role="status">{(*status).clone()}</p>
            }
            {error_line(&error)}
            <form class="card create-card" onsubmit={on_change}>
                <h2>{"Change password"}</h2>
                <input type="email" autocomplete="username" value={user.email.clone()} hidden=true readonly=true />
                <label>{"Current password"}<input type="password" autocomplete="current-password" required=true value={(*current).clone()} oninput={text_input(current.clone())} /></label>
                <label>{"New password"}<input type="password" autocomplete="new-password" required=true minlength="8" value={(*next).clone()} oninput={text_input(next.clone())} /></label>
                <button type="submit" disabled={*busy}>{if *busy {"Saving…"} else {"Change password"}}</button>
            </form>
            <section class="card create-card">
                <h2>{"Sessions"}</h2>
                <p class="muted">{"Signs this account out of every browser. API keys are not affected; revoke them on the API keys page."}</p>
                <button type="button" class="danger" onclick={on_everywhere}>{"Sign out everywhere"}</button>
            </section>
        </Shell>
    }
}
