//! Server-Sent Events that tell an open view its data changed.
//!
//! Background tabs disconnect, because browsers allow only a few
//! connections per site over HTTP/1.1; they reload once visible again.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::EventSource;
use yew::Callback;

type Handler = Closure<dyn FnMut(web_sys::Event)>;

struct Inner {
    url: String,
    source: RefCell<Option<EventSource>>,
    debounce: RefCell<Option<Timeout>>,
    on_change: Callback<()>,
    on_message: RefCell<Option<Handler>>,
    on_open: RefCell<Option<Handler>>,
    /// Whether a connection has opened before, so the next is a reconnect.
    opened: Cell<bool>,
}

impl Inner {
    /// Calls `on_change` shortly, collapsing bursts such as imports.
    fn schedule(&self) {
        let on_change = self.on_change.clone();
        *self.debounce.borrow_mut() = Some(Timeout::new(300, move || on_change.emit(())));
    }
    fn connect(&self) {
        if self.source.borrow().is_some() {
            return;
        }
        let Ok(source) = EventSource::new(&self.url) else {
            return;
        };
        if let Some(handler) = self.on_message.borrow().as_ref() {
            let _ =
                source.add_event_listener_with_callback("change", handler.as_ref().unchecked_ref());
        }
        if let Some(handler) = self.on_open.borrow().as_ref() {
            source.set_onopen(Some(handler.as_ref().unchecked_ref()));
        }
        *self.source.borrow_mut() = Some(source);
    }
    fn disconnect(&self) {
        if let Some(source) = self.source.borrow_mut().take() {
            source.set_onopen(None);
            source.close();
        }
    }
}

/// Listens for changes to a collection while alive.
pub struct Changes {
    inner: Rc<Inner>,
    on_visibility: Option<Handler>,
}

impl Changes {
    /// Calls `on_change` after changes, and after reconnecting or returning
    /// to the tab, when changes may have been missed.
    pub fn listen(collection_id: &str, on_change: Callback<()>) -> Self {
        let inner = Rc::new(Inner {
            url: format!("/api/v1/collections/{collection_id}/events"),
            source: RefCell::new(None),
            debounce: RefCell::new(None),
            on_change,
            on_message: RefCell::new(None),
            on_open: RefCell::new(None),
            opened: Cell::new(false),
        });
        let weak = Rc::downgrade(&inner);
        *inner.on_message.borrow_mut() = Some(Closure::new(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.schedule();
            }
        }));
        let weak = Rc::downgrade(&inner);
        *inner.on_open.borrow_mut() = Some(Closure::new(move |_| {
            if let Some(inner) = weak.upgrade()
                && inner.opened.replace(true)
            {
                inner.schedule();
            }
        }));
        let weak = Rc::downgrade(&inner);
        let on_visibility: Handler = Closure::new(move |_| {
            let Some(inner) = weak.upgrade() else { return };
            if hidden() {
                inner.disconnect();
            } else {
                inner.connect();
            }
        });
        let document = web_sys::window().and_then(|w| w.document());
        if let Some(document) = &document {
            let _ = document.add_event_listener_with_callback(
                "visibilitychange",
                on_visibility.as_ref().unchecked_ref(),
            );
        }
        if !hidden() {
            inner.connect();
        }
        Self {
            inner,
            on_visibility: Some(on_visibility),
        }
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        self.inner.disconnect();
        if let (Some(handler), Some(document)) = (
            self.on_visibility.take(),
            web_sys::window().and_then(|w| w.document()),
        ) {
            let _ = document.remove_event_listener_with_callback(
                "visibilitychange",
                handler.as_ref().unchecked_ref(),
            );
        }
    }
}

/// Whether the page is in a background tab.
pub fn hidden() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .is_some_and(|d| d.hidden())
}
