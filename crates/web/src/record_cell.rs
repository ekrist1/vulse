use crate::api;
use vulse_model::{
    DocumentPatch, DocumentView, FieldDef, FieldType, MutationResult, display_value,
};
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct CellProps {
    pub collections: Vec<vulse_model::CollectionView>,
    pub collection_id: String,
    pub document: DocumentView,
    pub field: FieldDef,
    pub on_saved: Callback<MutationResult>,
}
#[function_component(RecordCell)]
pub fn record_cell(props: &CellProps) -> Html {
    let editing = use_state(|| false);
    let draft = use_state(String::new);
    let base = use_state(String::new);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let value = display_value(&props.document.body[&props.field.name]);
    let calculated_error = props.document.calculation_errors.get(&props.field.name);
    let start = Callback::from({
        let editing = editing.clone();
        let draft = draft.clone();
        let base = base.clone();
        let error = error.clone();
        let version = props.document.updated_at.clone();
        let value = value.clone();
        move |_| {
            draft.set(value.clone());
            base.set(version.clone());
            error.set(String::new());
            editing.set(true);
        }
    });
    let save = Callback::from({
        let draft = draft.clone();
        let base = base.clone();
        let field = props.field.clone();
        let collection = props.collection_id.clone();
        let id = props.document.id.clone();
        let busy = busy.clone();
        let error = error.clone();
        let editing = editing.clone();
        let saved = props.on_saved.clone();
        move |()| {
            if *busy {
                return;
            }
            let value = match field.kind {
                FieldType::Number => {
                    if draft.trim().is_empty() {
                        serde_json::Value::Null
                    } else {
                        match serde_json::from_str::<serde_json::Value>(&draft) {
                            Ok(value) if value.is_number() => value,
                            _ => {
                                error.set("Enter a valid number".into());
                                return;
                            }
                        }
                    }
                }
                FieldType::Boolean => serde_json::json!(draft.eq_ignore_ascii_case("true")),
                FieldType::Date if draft.is_empty() => serde_json::Value::Null,
                _ => serde_json::json!((*draft).clone()),
            };
            let body = DocumentPatch {
                patch: serde_json::json!({field.name.clone():value}),
                expected_updated_at: Some((*base).clone()),
            };
            let collection = collection.clone();
            let id = id.clone();
            let busy = busy.clone();
            let error = error.clone();
            let editing = editing.clone();
            let saved = saved.clone();
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::patch_record(&collection, &id, &body).await {
                    Ok(result) => {
                        editing.set(false);
                        saved.emit(result);
                    }
                    Err(message) => error.set(message),
                }
                busy.set(false);
            });
        }
    });
    let cancel = Callback::from({
        let editing = editing.clone();
        let error = error.clone();
        move |()| {
            editing.set(false);
            error.set(String::new());
        }
    });
    let editable = matches!(
        props.field.kind,
        FieldType::String | FieldType::Number | FieldType::Boolean | FieldType::Date
    );
    html! {<td class={if calculated_error.is_some(){"cell-error"}else if props.field.kind==FieldType::Calculated{"cell-calculated"}else{""}} title={calculated_error.cloned().unwrap_or_else(||value.clone())}>
        if *editing {<div class="inline-cell">
            if props.field.kind==FieldType::Boolean {<select aria-label={format!("Edit {}",props.field.display_label())} disabled={*busy} onchange={Callback::from({let draft=draft.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();draft.set(input.value());}})}><option value="true" selected={draft.eq_ignore_ascii_case("true")}>{"True"}</option><option value="false" selected={!draft.eq_ignore_ascii_case("true")}>{"False"}</option></select>}
            else {<input aria-label={format!("Edit {}",props.field.display_label())} type={match props.field.kind{FieldType::Number=>"number",FieldType::Date=>"date",_=>"text"}} step="any" disabled={*busy} value={(*draft).clone()} oninput={Callback::from({let draft=draft.clone();move |e:InputEvent|{let input:HtmlInputElement=e.target_unchecked_into();draft.set(input.value());}})} onkeydown={Callback::from({let save=save.clone();let cancel=cancel.clone();move |e:KeyboardEvent|{if e.key()=="Enter"{e.prevent_default();save.emit(());}else if e.key()=="Escape"{e.prevent_default();cancel.emit(());}}})}/>}
            <div class="row"><button type="button" class="secondary" disabled={*busy} aria-label={format!("Save {} cell",props.field.display_label())} onclick={Callback::from(move |_|save.emit(()))}>{"Save"}</button><button type="button" class="text-button" disabled={*busy} onclick={Callback::from(move |_|cancel.emit(()))}>{"Cancel"}</button></div>
            if !error.is_empty(){<p class="error" role="alert">{(*error).clone()}</p>}
        </div>} else if editable {<button type="button" class="cell-edit" aria-label={format!("Edit {} for {}",props.field.display_label(),props.document.id)} onclick={start}>{if value.is_empty(){"—".into()}else{value}}<span aria-hidden="true">{" ✎"}</span></button>}
        else if props.field.kind==FieldType::Reference || (props.field.kind==FieldType::Array && props.field.items==Some(vulse_model::ArrayItems::Reference)) {<crate::relationships::ReferenceLinks collection={props.collections.iter().find(|c|Some(c.name.as_str())==props.field.collection.as_deref()).cloned()} ids={if let Some(id)=props.document.body[&props.field.name].as_str(){vec![id.to_string()]}else{props.document.body[&props.field.name].as_array().map(|a|a.iter().filter_map(|v|v.as_str().map(str::to_string)).collect()).unwrap_or_default()}}/>}
        else {{calculated_error.cloned().unwrap_or_else(||if value.is_empty(){"—".into()}else{value})}}
    </td>}
}
