use crate::api;
use vulse_model::{CollectionView, FieldType, ImportResult};
use web_sys::{File, HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct ImportPanelProps {
    pub collection: CollectionView,
    pub on_import: Callback<()>,
}
#[function_component(ImportPanel)]
pub fn import_panel(props: &ImportPanelProps) -> Html {
    let file = use_state(|| None::<File>);
    let result = use_state(|| None::<ImportResult>);
    let mapping = use_state(Vec::<Option<String>>::new);
    let sheet = use_state(String::new);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let validated = use_state(|| false);
    let success = use_state(|| 0usize);
    let completed = use_state(|| false);
    let mode = use_state(|| "append".to_string());
    let match_field = use_state(String::new);
    let has_headers = use_state(|| true);
    let run = {
        let file = file.clone();
        let result = result.clone();
        let mapping = mapping.clone();
        let sheet = sheet.clone();
        let error = error.clone();
        let busy = busy.clone();
        let validated = validated.clone();
        let success = success.clone();
        let mode = mode.clone();
        let match_field = match_field.clone();
        let has_headers = has_headers.clone();
        let completed = completed.clone();
        let id = props.collection.id.clone();
        Callback::from(move |commit: bool| {
            if *busy || (commit && !*validated) {
                return;
            }
            let Some(file) = (*file).clone() else {
                return;
            };
            busy.set(true);
            error.set(String::new());
            success.set(0);
            let initial = result.is_none();
            let targets = if initial {
                None
            } else {
                Some((*mapping).clone())
            };
            let result = result.clone();
            let mapping = mapping.clone();
            let sheet_state = sheet.clone();
            let selected = (*sheet).clone();
            let error = error.clone();
            let busy = busy.clone();
            let validated = validated.clone();
            let success = success.clone();
            let mode = (*mode).clone();
            let match_field = (*match_field).clone();
            let has_headers = *has_headers;
            let completed = completed.clone();
            let id = id.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let options = api::ImportOptions {
                    mapping: targets.as_deref(),
                    sheet: &selected,
                    commit,
                    mode: &mode,
                    match_field: &match_field,
                    has_headers,
                };
                match api::import_file(&id, &file, options).await {
                    Ok(next) => {
                        mapping.set(next.mapping.clone());
                        sheet_state.set(next.sheet.clone());
                        validated.set(next.errors.is_empty() && next.row_count > 0 && !commit);
                        completed.set(commit && next.errors.is_empty());
                        success.set(next.imported);
                        result.set(Some(next));
                    }
                    Err(e) => {
                        error.set(e);
                        validated.set(false);
                    }
                }
                busy.set(false);
            });
        })
    };
    let on_file = {
        let file = file.clone();
        let result = result.clone();
        let mapping = mapping.clone();
        let sheet = sheet.clone();
        let validated = validated.clone();
        let error = error.clone();
        let success = success.clone();
        let completed = completed.clone();
        Callback::from(move |e: Event| {
            let input: HtmlInputElement = e.target_unchecked_into();
            file.set(input.files().and_then(|f| f.get(0)));
            result.set(None);
            mapping.set(vec![]);
            sheet.set(String::new());
            validated.set(false);
            error.set(String::new());
            success.set(0);
            completed.set(false);
        })
    };
    html! {
        <section class="card import-panel">
            <div class="section-heading"><div><p class="eyebrow">{"BRING YOUR DATA"}</p><h2>{"From spreadsheet to collection"}</h2><p class="muted">{"Choose a file, map its columns, and review before importing."}</p></div><span class="badge">{"CSV · XLSX"}</span></div>
            <ol class="steps"><li class="active">{"1. Upload"}</li><li class={if result.is_some(){"active"}else{""}}>{"2. Map & validate"}</li><li class={if *validated || *completed{"active"}else{""}}>{"3. Import"}</li></ol>
            <label class="upload-zone"><strong>{"Choose your spreadsheet"}</strong><span class="muted">{"Up to 50 MB, 50,000 rows and 200 columns."}</span><input type="file" accept=".csv,.xlsx" disabled={*busy} onchange={on_file}/></label>
            <label class="check"><input type="checkbox" checked={*has_headers} disabled={*busy||*completed} onchange={Callback::from({let has_headers=has_headers.clone();let result=result.clone();let mapping=mapping.clone();let validated=validated.clone();move |e:Event|{has_headers.set(e.target_unchecked_into::<HtmlInputElement>().checked());result.set(None);mapping.set(vec![]);validated.set(false);}})}/>{"First row contains column headers"}</label>
            <div class="field-metadata"><label>{"Import behavior"}<select aria-label="Import behavior" disabled={*busy||*completed} onchange={Callback::from({let mode=mode.clone();let validated=validated.clone();move |e:Event|{mode.set(e.target_unchecked_into::<HtmlSelectElement>().value());validated.set(false);}})}><option value="append" selected={*mode=="append"}>{"Add new records"}</option><option value="upsert" selected={*mode=="upsert"}>{"Create or update by a unique field"}</option></select></label>
            if *mode=="upsert"{<label>{"Match existing records by"}<select aria-label="Match existing records by" disabled={*busy||*completed} onchange={Callback::from({let match_field=match_field.clone();let validated=validated.clone();move |e:Event|{match_field.set(e.target_unchecked_into::<HtmlSelectElement>().value());validated.set(false);}})}><option value="" selected={match_field.is_empty()}>{"Choose a unique field, such as SKU or email"}</option>{for props.collection.blueprint.fields.iter().filter(|f|matches!(f.kind,FieldType::String|FieldType::Number)).map(|f|html!{<option value={f.name.clone()} selected={*match_field==f.name}>{f.display_label()}</option>})}</select></label>}
            </div>
            <p class="muted">{"Imports add new records. Empty cells leave new fields unset and keep existing values during updates. Unmapped fields are preserved. Matching values must be unique; duplicate keys stop the entire import. Arrays, JSON and rich text use JSON. Calculated fields are generated automatically. Excel formulas use the workbook’s saved values."}</p>
            if !error.is_empty(){<p class="error" role="alert">{(*error).clone()}</p>}
            if let Some(next)=result.as_ref() {
                if !next.sheets.is_empty(){<label>{"Worksheet"}<select aria-label="Worksheet" disabled={*busy || *completed} onchange={Callback::from({let sheet=sheet.clone();let result=result.clone();let validated=validated.clone();let mapping=mapping.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();sheet.set(input.value());result.set(None);mapping.set(vec![]);validated.set(false);}})}>{for next.sheets.iter().map(|s|html!{<option value={s.clone()} selected={**sheet==*s}>{s.clone()}</option>})}</select></label>}
                <h3>{"Match columns to fields"}</h3><div class="mapping-grid">{for next.headers.iter().enumerate().map(|(i,h)|{
                    let mapping=mapping.clone();let validated=validated.clone();html!{<label>{h.clone()}<select aria-label={h.clone()} disabled={*busy || *completed} onchange={Callback::from({let mapping=mapping.clone();move |e:Event|{let input:HtmlSelectElement=e.target_unchecked_into();let mut m=(*mapping).clone();m[i]=if input.value().is_empty(){None}else{Some(input.value())};mapping.set(m);validated.set(false);}})}><option value="" selected={mapping.get(i).is_none_or(Option::is_none)}>{"Skip column"}</option>{for props.collection.blueprint.fields.iter().filter(|f|f.kind!=FieldType::Calculated).map(|f|html!{<option value={f.name.clone()} selected={mapping.get(i).and_then(Option::as_deref)==Some(f.name.as_str())}>{format!("{} [{}]",f.display_label(),f.name)}</option>})}</select></label>}
                })}</div>
                if !next.errors.is_empty(){<div class="error" role="alert"><strong>{if next.errors.iter().any(|e|e.row==0&&e.message.starts_with("…and")){"More than 500 issues to resolve".to_string()}else{format!("{} issues to resolve",next.errors.len())}}</strong><p>{"No records have been imported. Correct the mapping or source file, then validate again."}</p><ul class="import-errors">{for next.errors.iter().take(100).map(|e|html!{<li>{if e.row==0{e.message.clone()}else{format!("Row {}{}: {}",e.row,e.field.as_ref().map(|f|format!(", {f}")).unwrap_or_default(),e.message)}}</li>})}</ul>if next.errors.len()>100{<p>{"Showing the first 100 issues."}</p>}</div>}
                if next.errors.is_empty(){<p class="import-summary" role="status">{format!("{} new · {} updates · {} unchanged",next.created,next.updated,next.unchanged)}</p>}
                if !next.preview.is_empty(){<h3>{format!("Preview · first {} of {} rows",next.preview.len(),next.row_count)}</h3><div class="table-scroll"><table><thead><tr><th>{"Action"}</th>{for props.collection.blueprint.fields.iter().map(|f|html!{<th>{f.display_label()}</th>})}</tr></thead><tbody>{for next.preview.iter().enumerate().map(|(index,row)|html!{<tr><td><span class="badge">{next.preview_actions.get(index).cloned().unwrap_or_default()}</span></td>{for props.collection.blueprint.fields.iter().map(|f|html!{<td title={vulse_model::text_value(&row[&f.name])}>{vulse_model::text_value(&row[&f.name])}</td>})}</tr>})}</tbody></table></div>}
            }
            if *completed{<div class="success" role="status"><strong>{format!("✓ Import complete · {} records written",*success)}</strong><p>{"Unchanged rows were left as they were. Choose another file above to start a new import."}</p><button onclick={Callback::from({let on_import=props.on_import.clone();move |_|on_import.emit(())})}>{"View records →"}</button></div>}
            else{<div class="row"><button class="secondary" disabled={file.is_none() || *busy || (*mode=="upsert" && match_field.is_empty())} onclick={Callback::from({let run=run.clone();move |_|run.emit(false)})}>{if *busy{"Processing…"}else if result.is_none(){"Read file & preview"}else{"Validate & preview"}}</button><button disabled={!*validated || *busy} onclick={Callback::from(move |_|run.emit(true))}>{result.as_ref().map(|r|format!("Import {} records",r.row_count)).unwrap_or_else(||"Import records".into())}</button><span class="muted">{if *validated{"✓ All rows valid. Ready to import."}else{"Validate your data to enable import."}}</span></div>}
        </section>
    }
}
