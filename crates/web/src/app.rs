use vulse_model::{Role, UserView};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::pages::{
    AcceptPage, AccountPage, CollectionPage, CollectionsPage, DocumentPage, KeysPage, LoginPage,
    PeoplePage, SetupPage,
};

#[derive(Clone, PartialEq)]
pub struct SessionState {
    pub user: Option<UserView>,
    pub loaded: bool,
}

#[derive(Clone, PartialEq)]
pub struct SessionCtx {
    pub state: SessionState,
    pub set: Callback<SessionState>,
}

impl SessionCtx {
    /// The signed-in role, or viewer while signed out. The server enforces
    /// roles; the UI only hides controls that would be refused.
    pub fn role(&self) -> Role {
        self.state
            .user
            .as_ref()
            .map(|user| user.role)
            .unwrap_or(Role::Viewer)
    }

    pub fn can_edit(&self) -> bool {
        self.role() >= Role::Editor
    }

    pub fn is_admin(&self) -> bool {
        self.role() == Role::Admin
    }
}

#[derive(Clone, Routable, PartialEq)]
pub enum Route {
    #[at("/")]
    Home,
    #[at("/login")]
    Login,
    #[at("/setup")]
    Setup,
    #[at("/accept")]
    Accept,
    #[at("/account")]
    Account,
    #[at("/people")]
    People,
    #[at("/keys")]
    Keys,
    #[at("/collections/:id")]
    Collection { id: String },
    #[at("/collections/:id/documents/:doc_id")]
    Document { id: String, doc_id: String },
    #[not_found]
    #[at("/404")]
    NotFound,
}

#[function_component(App)]
pub fn app() -> Html {
    let session = use_state(|| SessionState {
        user: None,
        loaded: false,
    });
    {
        let session = session.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let user = api::me().await.ok();
                session.set(SessionState { user, loaded: true });
            });
        });
    }
    let set = {
        let session = session.clone();
        Callback::from(move |next: SessionState| session.set(next))
    };
    let ctx = SessionCtx {
        state: (*session).clone(),
        set,
    };
    html! {
        <ContextProvider<SessionCtx> context={ctx}>
            <BrowserRouter>
                <Switch<Route> render={switch} />
            </BrowserRouter>
        </ContextProvider<SessionCtx>>
    }
}

fn switch(route: Route) -> Html {
    match route {
        Route::Home => html! { <CollectionsPage /> },
        Route::Login => html! { <LoginPage /> },
        Route::Setup => html! { <SetupPage /> },
        Route::Accept => html! { <AcceptPage /> },
        Route::Account => html! { <AccountPage /> },
        Route::People => html! { <PeoplePage /> },
        Route::Keys => html! { <KeysPage /> },
        Route::Collection { id } => html! { <CollectionPage key={id.clone()} id={id.clone()} /> },
        Route::Document { id, doc_id } => {
            html! { <DocumentPage key={format!("{id}/{doc_id}")} id={id.clone()} doc_id={doc_id.clone()} /> }
        }
        Route::NotFound => html! { <Shell><h1>{"Page not found"}</h1></Shell> },
    }
}

#[derive(Properties, PartialEq)]
pub struct ShellProps {
    pub children: Children,
}

#[function_component(Shell)]
pub fn shell(props: &ShellProps) -> Html {
    let session = use_context::<SessionCtx>();
    let navigator = use_navigator();
    let logout = {
        let session = session.clone();
        let navigator = navigator.clone();
        Callback::from(move |_| {
            let session = session.clone();
            let navigator = navigator.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let _ = api::logout().await;
                if let Some(session) = session {
                    session.set.emit(SessionState {
                        user: None,
                        loaded: true,
                    });
                }
                if let Some(navigator) = navigator {
                    navigator.push(&Route::Login);
                }
            });
        })
    };
    let user = session
        .as_ref()
        .and_then(|session| session.state.user.clone());
    html! {
        <div class="app">
            <header class="topbar">
                <Link<Route> to={Route::Home}><span class="brand-icon">{"v"}</span><span class="wordmark">{"vulse"}</span></Link<Route>>
                if let Some(user) = user {
                    <nav>
                        <Link<Route> to={Route::Home}>{"Collections"}</Link<Route>>
                        <Link<Route> to={Route::Keys}>{"API keys"}</Link<Route>>
                        if user.role == Role::Admin {
                            <Link<Route> to={Route::People}>{"People"}</Link<Route>>
                        }
                        <Link<Route> classes="handle" to={Route::Account}>{format!("@{}", user.handle)}</Link<Route>>
                        <button type="button" onclick={logout}>{"Log out"}</button>
                    </nav>
                }
            </header>
            <main>{for props.children.iter()}</main><footer class="app-footer"><span>{"Vulse workspace"}</span><span>{"A little structure. Endless possibilities."}</span></footer>
        </div>
    }
}

pub fn signed_out(session: &SessionCtx) -> Option<Html> {
    if !session.state.loaded {
        Some(html! { <Shell><p class="muted">{"Loading…"}</p></Shell> })
    } else if session.state.user.is_none() {
        Some(html! { <Redirect<Route> to={Route::Login} /> })
    } else {
        None
    }
}

pub fn confirm(message: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(message).ok())
        .unwrap_or(false)
}
