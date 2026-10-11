use vulse_model::{LoginRequest, RegisterRequest};
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
    let session = use_context::<SessionCtx>().expect("session");
    let navigator = use_navigator().unwrap();
    if session.state.user.is_some() {
        return html! { <Redirect<Route> to={Route::Home} /> };
    }
    let onsubmit = {
        let email = email.clone();
        let password = password.clone();
        let error = error.clone();
        let session = session.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let email = (*email).clone();
            let password = (*password).clone();
            let error = error.clone();
            let session = session.clone();
            let navigator = navigator.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::login(&LoginRequest { email, password }).await {
                    Ok(user) => {
                        session.set.emit(SessionState {
                            user: Some(user),
                            loaded: true,
                        });
                        navigator.push(&Route::Home);
                    }
                    Err(err) => error.set(err),
                }
            });
        })
    };
    html! {
        <Shell>
            <h1>{"Log in"}</h1>
            <form onsubmit={onsubmit}>
                <label>{"Email"}<input type="email" required=true value={(*email).clone()} oninput={text_input(email)} /></label>
                <label>{"Password"}<input type="password" required=true value={(*password).clone()} oninput={text_input(password)} /></label>
                {error_line(&error)}
                <button type="submit">{"Log in"}</button>
            </form>
            <p class="muted"><Link<Route> to={Route::Register}>{"Create an account"}</Link<Route>></p>
        </Shell>
    }
}

#[function_component(RegisterPage)]
pub fn register_page() -> Html {
    let email = use_state(String::new);
    let handle = use_state(String::new);
    let password = use_state(String::new);
    let error = use_state(String::new);
    let session = use_context::<SessionCtx>().expect("session");
    let navigator = use_navigator().unwrap();
    let onsubmit = {
        let email = email.clone();
        let handle = handle.clone();
        let password = password.clone();
        let error = error.clone();
        let session = session.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let body = RegisterRequest {
                email: (*email).clone(),
                handle: (*handle).clone(),
                password: (*password).clone(),
            };
            let error = error.clone();
            let session = session.clone();
            let navigator = navigator.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::register(&body).await {
                    Ok(user) => {
                        session.set.emit(SessionState {
                            user: Some(user),
                            loaded: true,
                        });
                        navigator.push(&Route::Home);
                    }
                    Err(err) => error.set(err),
                }
            });
        })
    };
    html! {
        <Shell>
            <h1>{"Create an account"}</h1>
            <form onsubmit={onsubmit}>
                <label>{"Email"}<input type="email" required=true value={(*email).clone()} oninput={text_input(email)} /></label>
                <label>{"Handle"}<input required=true value={(*handle).clone()} oninput={text_input(handle)} /></label>
                <label>{"Password"}<input type="password" required=true minlength="8" value={(*password).clone()} oninput={text_input(password)} /></label>
                {error_line(&error)}
                <button type="submit">{"Create account"}</button>
            </form>
            <p class="muted"><Link<Route> to={Route::Login}>{"Already have an account?"}</Link<Route>></p>
        </Shell>
    }
}

fn text_input(state: UseStateHandle<String>) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| {
        let element: HtmlInputElement = event.target_unchecked_into();
        state.set(element.value());
    })
}

fn error_line(error: &UseStateHandle<String>) -> Html {
    if error.is_empty() {
        html! {}
    } else {
        html! { <p class="error" role="alert">{(**error).clone()}</p> }
    }
}
