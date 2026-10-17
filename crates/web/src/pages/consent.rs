use vulse_model::{CollectionView, OAuthConsent, OAuthRequestView};
use web_sys::HtmlSelectElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::app::{Route, SessionCtx, Shell};

/// Where to continue after signing in, kept for this browser tab only.
pub(crate) const RETURN_KEY: &str = "vulse_return";
const CONSENT_PATH: &str = "/oauth/consent";

/// The consent path saved before signing in, if any. Taken once.
pub(crate) fn take_return_path() -> Option<String> {
    let storage = web_sys::window()?.session_storage().ok()??;
    let path = storage.get_item(RETURN_KEY).ok()??;
    let _ = storage.remove_item(RETURN_KEY);
    path.starts_with(&format!("{CONSENT_PATH}?")).then_some(path)
}

fn remember_return_path() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let location = window.location();
    let path = format!(
        "{}{}",
        location.pathname().unwrap_or_default(),
        location.search().unwrap_or_default()
    );
    if let Ok(Some(storage)) = window.session_storage() {
        let _ = storage.set_item(RETURN_KEY, &path);
    }
}

fn request_id() -> String {
    let search = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .unwrap_or_default();
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix("request="))
        .unwrap_or_default()
        .to_string()
}

/// Lets a signed-in user allow or deny an app, such as Claude, that wants to
/// use Vulse through MCP.
#[function_component(OAuthConsentPage)]
pub fn oauth_consent_page() -> Html {
    let session = use_context::<SessionCtx>().expect("session");
    let request = use_memo((), |_| request_id());
    let info = use_state(|| None::<OAuthRequestView>);
    let collections = use_state(Vec::<CollectionView>::new);
    let write = use_state(|| true);
    let collection_id = use_state(String::new);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    {
        let request = request.clone();
        let info = info.clone();
        let collections = collections.clone();
        let error = error.clone();
        let authed = session.state.user.is_some();
        use_effect_with(authed, move |authed| {
            if *authed {
                wasm_bindgen_futures::spawn_local(async move {
                    match api::oauth_request(&request).await {
                        Ok(found) => info.set(Some(found)),
                        Err(err) => error.set(err),
                    }
                    if let Ok(items) = api::collections().await {
                        collections.set(items);
                    }
                });
            }
        });
    }
    if !session.state.loaded {
        return html! { <Shell><p class="muted">{"Loading…"}</p></Shell> };
    }
    let Some(user) = session.state.user.clone() else {
        remember_return_path();
        return html! { <Redirect<Route> to={Route::Login} /> };
    };
    let decide = |approve: bool| {
        let request = request.clone();
        let info = info.clone();
        let write = write.clone();
        let collection_id = collection_id.clone();
        let error = error.clone();
        let busy = busy.clone();
        Callback::from(move |_| {
            let write_granted = info
                .as_ref()
                .is_some_and(|info| info.write_requested && info.write_allowed)
                && *write;
            let body = OAuthConsent {
                request: (*request).clone(),
                approve,
                write: write_granted,
                collection_id: (!collection_id.is_empty()).then(|| (*collection_id).clone()),
            };
            let error = error.clone();
            let busy = busy.clone();
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::oauth_consent(&body).await {
                    Ok(next) => {
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href(&next.redirect);
                        }
                    }
                    Err(err) => {
                        error.set(err);
                        busy.set(false);
                    }
                }
            });
        })
    };
    let body = match &*info {
        None if error.is_empty() => html! { <p class="muted">{"Loading…"}</p> },
        None => html! {},
        Some(info) => html! {
            <section class="card">
                <p>
                    <strong>{info.client_name.clone()}</strong>
                    {info.client_domain.as_ref().map(|domain| format!(" (from {domain})")).unwrap_or_default()}
                    {format!(" wants to use Vulse as @{}. After you decide, you return to {}.", user.handle, info.redirect_host)}
                </p>
                if info.write_requested && info.write_allowed {
                    <label>{"Access"}
                        <select onchange={Callback::from({
                            let write = write.clone();
                            move |event: Event| {
                                let element: HtmlSelectElement = event.target_unchecked_into();
                                write.set(element.value() == "write");
                            }
                        })}>
                            <option value="write" selected={*write}>{"Read and change records"}</option>
                            <option value="read" selected={!*write}>{"Only read records"}</option>
                        </select>
                    </label>
                } else if info.write_requested {
                    <p class="muted">{"Your role is Viewer, so the app can only read records."}</p>
                } else {
                    <p class="muted">{"The app asks to read records only."}</p>
                }
                <label>{"Collections"}
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
                <p class="muted">{"The app can never change collection fields, people or keys. You can disconnect it at any time under API keys."}</p>
                <div class="row">
                    <button type="button" disabled={*busy} onclick={decide(true)}>{"Allow"}</button>
                    <button type="button" class="secondary" disabled={*busy} onclick={decide(false)}>{"Deny"}</button>
                </div>
            </section>
        },
    };
    html! {
        <Shell>
            <h1>{"Connect an app"}</h1>
            if !error.is_empty() {
                <p class="error" role="alert">{(*error).clone()}</p>
            }
            {body}
        </Shell>
    }
}
