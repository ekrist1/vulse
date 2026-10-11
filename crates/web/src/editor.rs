use serde_json::Value;
use wasm_bindgen::{closure::Closure, prelude::*};
use yew::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "VulseEditor"], js_name = mount)]
    fn mount_editor(
        element: &web_sys::Element,
        collection: &str,
        label: &str,
        json: &str,
        change: &JsValue,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_namespace = ["window", "VulseEditor"], js_name = update)]
    fn update_editor(handle: &JsValue, json: &str);
    #[wasm_bindgen(js_namespace = ["window", "VulseEditor"], js_name = destroy)]
    fn destroy_editor(handle: &JsValue);
}

#[derive(Properties, PartialEq)]
pub struct RichEditorProps {
    pub collection_id: String,
    pub label: String,
    pub value: Value,
    pub on_change: Callback<Value>,
}

#[function_component(RichEditor)]
pub fn rich_editor(props: &RichEditorProps) -> Html {
    let node = use_node_ref();
    let handle = use_mut_ref(|| None::<JsValue>);
    let callback = use_mut_ref(|| props.on_change.clone());
    *callback.borrow_mut() = props.on_change.clone();
    let error = use_state(String::new);
    {
        let node = node.clone();
        let handle = handle.clone();
        let callback = callback.clone();
        let error = error.clone();
        let value = props.value.to_string();
        let label = props.label.clone();
        use_effect_with(props.collection_id.clone(), move |collection| {
            let change = Closure::<dyn FnMut(String)>::new(move |json: String| {
                if let Ok(value) = serde_json::from_str(&json) {
                    callback.borrow().emit(value);
                }
            });
            if let Some(element) = node.cast::<web_sys::Element>() {
                match mount_editor(&element, collection, &label, &value, change.as_ref()) {
                    Ok(editor) => *handle.borrow_mut() = Some(editor),
                    Err(_) => error.set(
                        "The editor could not load. Reload the page before editing this field."
                            .into(),
                    ),
                }
            }
            move || {
                if let Some(editor) = handle.borrow_mut().take() {
                    destroy_editor(&editor);
                }
                drop(change);
            }
        });
    }
    {
        let handle = handle.clone();
        use_effect_with(props.value.clone(), move |value| {
            if let Some(editor) = handle.borrow().as_ref() {
                update_editor(editor, &value.to_string());
            }
        });
    }
    html! {<div><div ref={node}/>{if error.is_empty(){html!{}}else{html!{<p class="error" role="alert">{(*error).clone()}</p>}}}</div>}
}
