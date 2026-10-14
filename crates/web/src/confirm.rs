//! In-app confirmation modal for destructive actions.
//!
//! Components call `use_confirm()` and `await` the answer:
//!
//! ```ignore
//! let confirm = use_confirm();
//! spawn_local(async move {
//!     if !confirm.ask(ConfirmRequest::danger("Delete record?", "…", "Delete record")).await {
//!         return;
//!     }
//!     // …
//! });
//! ```

use futures_channel::oneshot;
use web_sys::HtmlDialogElement;
use yew::prelude::*;

#[derive(Clone, PartialEq)]
pub struct ConfirmRequest {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
    /// Styles the confirm button as destructive and focuses Cancel first.
    pub danger: bool,
}

impl ConfirmRequest {
    pub fn danger(
        title: impl Into<String>,
        message: impl Into<String>,
        confirm_label: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            confirm_label: confirm_label.into(),
            danger: true,
        }
    }

    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        confirm_label: impl Into<String>,
    ) -> Self {
        Self {
            danger: false,
            ..Self::danger(title, message, confirm_label)
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Confirmer {
    open: Callback<(ConfirmRequest, oneshot::Sender<bool>)>,
}

impl Confirmer {
    /// Shows the modal and resolves to `true` only if the user confirms.
    pub async fn ask(&self, request: ConfirmRequest) -> bool {
        let (sender, receiver) = oneshot::channel();
        self.open.emit((request, sender));
        receiver.await.unwrap_or(false)
    }
}

#[hook]
pub fn use_confirm() -> Confirmer {
    use_context::<Confirmer>().expect("ConfirmProvider is missing")
}

#[derive(Properties, PartialEq)]
pub struct ConfirmProviderProps {
    pub children: Children,
}

#[function_component(ConfirmProvider)]
pub fn confirm_provider(props: &ConfirmProviderProps) -> Html {
    let request = use_state(|| None::<ConfirmRequest>);
    let pending = use_mut_ref(|| None::<oneshot::Sender<bool>>);
    let dialog = use_node_ref();
    let answer = {
        let request = request.clone();
        let pending = pending.clone();
        Callback::from(move |accepted: bool| {
            if let Some(sender) = pending.borrow_mut().take() {
                let _ = sender.send(accepted);
            }
            request.set(None);
        })
    };
    let open = {
        let request = request.clone();
        let pending = pending.clone();
        Callback::from(
            move |(next, sender): (ConfirmRequest, oneshot::Sender<bool>)| {
                // A new question cancels one still on screen.
                if let Some(previous) = pending.borrow_mut().replace(sender) {
                    let _ = previous.send(false);
                }
                request.set(Some(next));
            },
        )
    };
    {
        let dialog = dialog.clone();
        use_effect_with((*request).clone(), move |request| {
            if let Some(element) = dialog.cast::<HtmlDialogElement>()
                && request.is_some() && !element.open() {
                    let _ = element.show_modal();
                }
        });
    }
    let confirmer = Confirmer { open };
    html! {
        <ContextProvider<Confirmer> context={confirmer}>
            {for props.children.iter()}
            if let Some(current) = (*request).clone() {
                <dialog
                    ref={dialog}
                    class="confirm-dialog"
                    aria-labelledby="confirm-title"
                    aria-describedby="confirm-message"
                    oncancel={Callback::from({
                        let answer = answer.clone();
                        move |event: Event| {
                            event.prevent_default();
                            answer.emit(false);
                        }
                    })}
                >
                    <h2 id="confirm-title">{current.title.clone()}</h2>
                    <p id="confirm-message">{current.message.clone()}</p>
                    <div class="confirm-actions">
                        <button
                            type="button"
                            class="secondary confirm-cancel"
                            autofocus={current.danger}
                            onclick={Callback::from({
                                let answer = answer.clone();
                                move |_| answer.emit(false)
                            })}
                        >{"Cancel"}</button>
                        <button
                            type="button"
                            class={classes!("confirm-accept", current.danger.then_some("danger-solid"))}
                            autofocus={!current.danger}
                            onclick={Callback::from({
                                let answer = answer.clone();
                                move |_| answer.emit(true)
                            })}
                        >{current.confirm_label.clone()}</button>
                    </div>
                </dialog>
            }
        </ContextProvider<Confirmer>>
    }
}
