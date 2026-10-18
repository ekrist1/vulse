use vulse_model::{BackupOverview, ChangePassword, Role};
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::app::{Route, SessionCtx, SessionState, Shell, signed_out};
use crate::confirm::{ConfirmRequest, use_confirm};
use crate::pages::auth::{error_line, text_input};

#[function_component(AccountPage)]
pub fn account_page() -> Html {
    let confirm = use_confirm();
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
            let session = session.clone();
            let error = error.clone();
            let navigator = navigator.clone();
            let confirm = confirm.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let request = ConfirmRequest::danger(
                    "Sign out everywhere?",
                    "Every browser and device signed in to this account is signed out, including this one. API keys keep working.",
                    "Sign out everywhere",
                );
                if !confirm.ask(request).await {
                    return;
                }
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
            if user.role == Role::Admin {
                <BackupsCard />
            }
            <section class="card create-card">
                <h2>{"Sessions"}</h2>
                <p class="muted">{"Signs this account out of every browser. API keys are not affected; revoke them on the API keys page."}</p>
                <button type="button" class="danger" onclick={on_everywhere}>{"Sign out everywhere"}</button>
            </section>
        </Shell>
    }
}

/// Admins make, download and delete backups of the whole workspace.
#[function_component(BackupsCard)]
fn backups_card() -> Html {
    let confirm = use_confirm();
    let overview = use_state(|| None::<BackupOverview>);
    let error = use_state(String::new);
    let status = use_state(String::new);
    let busy = use_state(|| false);
    let reload = {
        let overview = overview.clone();
        let error = error.clone();
        Callback::from(move |_: ()| {
            let overview = overview.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::backups().await {
                    Ok(next) => overview.set(Some(next)),
                    Err(err) => error.set(err),
                }
            });
        })
    };
    {
        let reload = reload.clone();
        use_effect_with((), move |_| reload.emit(()));
    }
    let on_create = {
        let (error, status, busy, reload) = (error.clone(), status.clone(), busy.clone(), reload.clone());
        Callback::from(move |_| {
            let (error, status, busy, reload) = (error.clone(), status.clone(), busy.clone(), reload.clone());
            busy.set(true);
            error.set(String::new());
            status.set(String::new());
            wasm_bindgen_futures::spawn_local(async move {
                match api::create_backup().await {
                    Ok(made) => {
                        status.set(format!("Backup created: {} ({})", made.name, size(made.bytes)));
                        reload.emit(());
                    }
                    Err(err) => error.set(err),
                }
                busy.set(false);
            });
        })
    };
    let schedule = match overview.as_ref() {
        Some(BackupOverview { every_hours: Some(hours), keep: Some(keep), .. }) => format!(
            "Scheduled every {} and keeps the newest {keep}, including the ones made here.",
            if *hours == 1 { "hour".to_string() } else { format!("{hours} hours") }
        ),
        Some(_) => "Scheduled backups are off. Set VULSE_BACKUP_DIR to turn them on.".to_string(),
        None => String::new(),
    };
    html! {
        <section class="card backups-card">
            <div class="section-heading">
                <div>
                    <h2>{"Backups"}</h2>
                    <p class="muted">{"A backup is one zip with the database and every uploaded file. Vulse keeps running while it is made. It holds all accounts and records, so store downloads somewhere safe."}</p>
                </div>
                <button type="button" disabled={*busy || overview.is_none()} onclick={on_create}>{if *busy {"Creating backup…"} else {"Create backup"}}</button>
            </div>
            if let Some(found) = overview.as_ref() {
                <p class="muted backup-location">{schedule}{" Saved on the server in "}<code>{found.dir.clone()}</code>{"."}</p>
            }
            if !status.is_empty() {
                <p class="success" role="status">{(*status).clone()}</p>
            }
            {error_line(&error)}
            if let Some(found) = overview.as_ref() {
                if found.backups.is_empty() {
                    <p class="muted">{"No backups yet."}</p>
                } else {
                    <ul class="list backup-list">
                        {for found.backups.iter().map(|backup| {
                            let on_delete = {
                                let (name, confirm, error, status, reload) = (backup.name.clone(), confirm.clone(), error.clone(), status.clone(), reload.clone());
                                Callback::from(move |_| {
                                    let (name, confirm, error, status, reload) = (name.clone(), confirm.clone(), error.clone(), status.clone(), reload.clone());
                                    wasm_bindgen_futures::spawn_local(async move {
                                        let request = ConfirmRequest::danger(
                                            "Delete this backup?",
                                            format!("{name} is removed from the server. Downloaded copies are not affected."),
                                            "Delete backup",
                                        );
                                        if !confirm.ask(request).await {
                                            return;
                                        }
                                        match api::delete_backup(&name).await {
                                            Ok(()) => {
                                                status.set(format!("Deleted {name}."));
                                                reload.emit(());
                                            }
                                            Err(err) => error.set(err),
                                        }
                                    });
                                })
                            };
                            html! {
                                <li key={backup.name.clone()}>
                                    <div>
                                        <strong>{when(&backup.created_at)}</strong>
                                        <span class="muted">{format!(" · {}", size(backup.bytes))}</span>
                                        <div class="muted backup-name">{backup.name.clone()}</div>
                                    </div>
                                    <div class="row">
                                        <a class="button secondary" href={api::backup_url(&backup.name)} download={backup.name.clone()}>{"Download"}</a>
                                        <button type="button" class="danger" disabled={*busy} onclick={on_delete}>{"Delete"}</button>
                                    </div>
                                </li>
                            }
                        })}
                    </ul>
                }
                <p class="muted backup-restore">{"Restoring replaces all data and is done on the server while Vulse is stopped. See Backups in the README."}</p>
            }
        </section>
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        b if b >= 1_000 => format!("{:.0} KB", b as f64 / 1e3),
        b => format!("{b} bytes"),
    }
}

/// `2026-10-18T14:09:29Z` as `2026-10-18 14:09 UTC`.
fn when(created_at: &str) -> String {
    match (created_at.get(..10), created_at.get(11..16)) {
        (Some(date), Some(time)) => format!("{date} {time} UTC"),
        _ => created_at.to_string(),
    }
}
