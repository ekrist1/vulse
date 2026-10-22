//! The SQL page: write or pick a statement, run it, and confirm changes
//! after reviewing a preview.

use vulse_model::{CollectionView, FieldType, Role, SqlRequest, SqlResult};
use web_sys::HtmlSelectElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::app::{Route, SessionCtx, Shell, signed_out};
use crate::confirm::{ConfirmRequest, use_confirm};
use crate::sql_editor::SqlEditor;

struct Template {
    title: &'static str,
    description: &'static str,
    writes: bool,
    sql: fn(&str, &str, &str) -> String,
}

/// Starting points, filled in with a collection and two of its fields.
const TEMPLATES: [Template; 7] = [
    Template {
        title: "Show records",
        description: "The first 50 records with all fields.",
        writes: false,
        sql: |c, _, _| format!("SELECT *\nFROM {c}\nLIMIT 50"),
    },
    Template {
        title: "Find records",
        description: "Records matching a condition, largest first.",
        writes: false,
        sql: |c, t, n| {
            format!("SELECT id, {t}, {n}\nFROM {c}\nWHERE {n} > 0\nORDER BY {n} DESC\nLIMIT 50")
        },
    },
    Template {
        title: "Count per value",
        description: "How many records share each value.",
        writes: false,
        sql: |c, t, _| {
            format!(
                "SELECT {t}, COUNT(*) AS records\nFROM {c}\nGROUP BY {t}\nORDER BY records DESC"
            )
        },
    },
    Template {
        title: "Totals per group",
        description: "Sum and average of a number for each value.",
        writes: false,
        sql: |c, t, n| {
            format!(
                "SELECT {t}, SUM({n}) AS total, ROUND(AVG({n}), 2) AS average\nFROM {c}\nGROUP BY {t}\nORDER BY total DESC"
            )
        },
    },
    Template {
        title: "Find empty values",
        description: "Records where a field has no value.",
        writes: false,
        sql: |c, t, _| format!("SELECT id, {t}\nFROM {c}\nWHERE {t} IS NULL"),
    },
    Template {
        title: "Update where…",
        description: "Set a field on the records that match. Previewed before anything changes.",
        writes: true,
        sql: |c, t, n| format!("UPDATE {c}\nSET {t} = 'New value'\nWHERE {n} < 2020"),
    },
    Template {
        title: "Delete where…",
        description: "Move matching records to the Trash. Previewed before anything changes.",
        writes: true,
        sql: |c, _, n| format!("DELETE FROM {c}\nWHERE {n} < 2020"),
    },
];

/// A text-like and a number-like field to fill templates with.
fn template_fields(collection: &CollectionView) -> (String, String) {
    let fields = &collection.blueprint.fields;
    let text = fields
        .iter()
        .find(|f| f.kind == FieldType::String)
        .or(fields.first())
        .map(|f| f.name.clone())
        .unwrap_or_else(|| "field".into());
    let number = fields
        .iter()
        .find(|f| {
            matches!(
                f.kind,
                FieldType::Number | FieldType::Calculated | FieldType::Date
            )
        })
        .or(fields.first())
        .map(|f| f.name.clone())
        .unwrap_or_else(|| "field".into());
    (text, number)
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", group_digits(n), if n == 1 { one } else { many })
}
pub(super) fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[function_component(SqlPage)]
pub fn sql_page() -> Html {
    let session = use_context::<SessionCtx>().expect("session");
    let can_write = session
        .state
        .user
        .as_ref()
        .is_some_and(|u| u.role != Role::Viewer);
    let collections = use_state(Vec::<CollectionView>::new);
    let chosen = use_state(String::new);
    // The statement survives reloads, like record drafts.
    let sql = use_state(|| {
        crate::drafts::read::<String>("vulse-sql")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let result = use_state(|| None::<(String, SqlResult)>);
    let error = use_state(String::new);
    let busy = use_state(|| false);
    let confirm = use_confirm();
    {
        let collections = collections.clone();
        let chosen = chosen.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(items) = api::collections().await {
                    if let Some(first) = items.first() {
                        chosen.set(first.name.clone());
                    }
                    collections.set(items);
                }
            });
        });
    }
    if let Some(redirect) = signed_out(&session) {
        return redirect;
    }
    let run = {
        let sql = sql.clone();
        let result = result.clone();
        let error = error.clone();
        let busy = busy.clone();
        Callback::from(move |_: ()| {
            let statement = (*sql).trim().to_string();
            if statement.is_empty() || *busy {
                return;
            }
            let (result, error, busy) = (result.clone(), error.clone(), busy.clone());
            busy.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::run_sql(&SqlRequest {
                    sql: statement.clone(),
                    confirm: None,
                })
                .await
                {
                    Ok(next) => {
                        result.set(Some((statement, next)));
                        error.set(String::new());
                    }
                    Err(message) => {
                        result.set(None);
                        error.set(message);
                    }
                }
                busy.set(false);
            });
        })
    };
    let on_change = {
        let sql = sql.clone();
        Callback::from(move |next: String| {
            let _ = crate::drafts::write("vulse-sql", &next);
            sql.set(next);
        })
    };
    let current = collections.iter().find(|c| c.name == *chosen).cloned();
    let templates = current.as_ref().map(|collection| {
        let (text, number) = template_fields(collection);
        html! {<div class="sql-templates">
            {for TEMPLATES.iter().filter(|t| can_write || !t.writes).map(|t| {
                let statement = (t.sql)(&collection.name, &text, &number);
                let on_change = on_change.clone();
                let result = result.clone();
                html!{<button type="button" class={if t.writes {"sql-template writes"} else {"sql-template"}} onclick={Callback::from(move |_| {on_change.emit(statement.clone()); result.set(None);})}>
                    <strong>{t.title}</strong><span>{t.description}</span>
                </button>}
            })}
        </div>}
    });
    // The preview only applies to the statement it was made for.
    let stale = result
        .as_ref()
        .is_some_and(|(statement, _)| statement.trim() != sql.trim());
    let apply = {
        let result = result.clone();
        let error = error.clone();
        let busy = busy.clone();
        let confirm = confirm.clone();
        Callback::from(move |_| {
            let Some((statement, preview)) = (*result).clone() else {
                return;
            };
            let Some(token) = preview.confirmation.clone() else {
                return;
            };
            let (result, error, busy, confirm) =
                (result.clone(), error.clone(), busy.clone(), confirm.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let count = plural(preview.affected, "record", "records");
                let request = if preview.kind == "delete" {
                    ConfirmRequest::danger(
                        format!("Delete {count} from {}?", preview.collection),
                        "They move to the collection's Trash, where each can be restored.",
                        format!("Delete {count}"),
                    )
                } else {
                    ConfirmRequest::danger(
                        format!("Update {count} in {}?", preview.collection),
                        "Each record's previous version stays in its history.",
                        format!("Update {count}"),
                    )
                };
                if !confirm.ask(request).await {
                    return;
                }
                busy.set(true);
                match api::run_sql(&SqlRequest {
                    sql: statement.clone(),
                    confirm: Some(token),
                })
                .await
                {
                    Ok(done) => {
                        result.set(Some((statement, done)));
                        error.set(String::new());
                    }
                    Err(message) => error.set(message),
                }
                busy.set(false);
            });
        })
    };
    html! {<Shell>
        <div class="page-heading"><div><p class="eyebrow">{"WORKSPACE"}</p><h1>{"SQL"}</h1>
            <p class="muted">{"Query a collection with SQL, or change many records at once. Changes are previewed and need confirmation, and go through the same checks, history and Trash as edits in the table."}</p></div></div>
        <div class="sql-layout">
            <aside class="card sql-side">
                <h2>{"Templates"}</h2>
                <label>{"Collection"}
                    <select aria-label="Template collection" onchange={Callback::from({let chosen=chosen.clone();move |e:Event|chosen.set(e.target_unchecked_into::<HtmlSelectElement>().value())})}>
                        {for collections.iter().map(|c| html!{<option value={c.name.clone()} selected={c.name == *chosen}>{c.name.clone()}</option>})}
                    </select>
                </label>
                {templates.unwrap_or_default()}
                if let Some(collection) = &current {
                    <h3>{"Fields"}</h3>
                    <ul class="sql-fields">
                        {for collection.blueprint.fields.iter().map(|f| html!{<li><code>{f.name.clone()}</code><span class="muted">{f.display_label().to_string()}</span></li>})}
                        <li><code>{"id, created_at, updated_at"}</code><span class="muted">{"record properties"}</span></li>
                    </ul>
                }
            </aside>
            <section class="sql-main">
                <div class="card">
                    <SqlEditor value={(*sql).clone()} on_change={on_change.clone()} on_run={run.clone()} collections={(*collections).clone()}/>
                    <div class="row sql-actions">
                        <button type="button" disabled={*busy || sql.trim().is_empty()} onclick={Callback::from({let run=run.clone();move |_|run.emit(())})}>{if *busy {"Running…"} else {"Run"}}</button>
                        <span class="muted">{"Ctrl + Enter runs · ↑ ↓ and Enter pick a suggestion · UPDATE and DELETE show a preview first"}</span>
                    </div>
                    if !can_write {<p class="muted">{"Your role can run SELECT statements. Ask an admin to make you an editor to change records."}</p>}
                </div>
                if !error.is_empty() {<p class="error" role="alert">{(*error).clone()}</p>}
                if let Some((_, r)) = result.as_ref() {
                    {render_result(r, stale, *busy, apply.clone(), {let result=result.clone();Callback::from(move |_|result.set(None))})}
                }
            </section>
        </div>
    </Shell>}
}

fn render_result(
    r: &SqlResult,
    stale: bool,
    busy: bool,
    apply: Callback<MouseEvent>,
    cancel: Callback<MouseEvent>,
) -> Html {
    let collection_link = html! {<Link<Route> to={Route::Collection { id: r.collection_id.clone() }}>{format!("Open {} →", r.collection)}</Link<Route>>};
    if r.kind == "select" {
        let shown = r.rows.len() as u64;
        return html! {<div class="card sql-result">
            <div class="section-heading"><h2>{plural(r.total, "row", "rows")}</h2>{collection_link}</div>
            if r.rows.is_empty() {<p class="muted">{"No records match."}</p>}
            else {<div class="table-scroll"><table class="sql-table"><thead><tr>{for r.columns.iter().map(|c| html!{<th>{c.clone()}</th>})}</tr></thead>
                <tbody>{for r.rows.iter().map(|row| html!{<tr>{for row.iter().map(|v| html!{<td title={vulse_model::display_value(v)}>{if v.is_null() {html!{<span class="muted">{"—"}</span>}} else {html!{vulse_model::display_value(v)}}}</td>})}</tr>})}</tbody></table></div>}
            if r.truncated {<p class="muted">{format!("Showing the first {} rows. Add LIMIT and OFFSET, or narrow WHERE, to see others.", group_digits(shown))}</p>}
        </div>};
    }
    let verb = if r.kind == "delete" {
        "Delete"
    } else {
        "Update"
    };
    let count = plural(r.affected, "record", "records");
    if r.done {
        return html! {<div class="success sql-done" role="status">
            {format!("{}d {count} in {}. ", verb, r.collection)}
            {if r.kind == "delete" {"Restore them from the collection's Trash if needed. "} else {"Previous versions are in each record's history. "}}
            {collection_link}
        </div>};
    }
    if r.affected == 0 {
        return html! {<div class="card sql-result"><h2>{"Nothing to change"}</h2><p class="muted">{"No records match, or they already have these values."}</p></div>};
    }
    let fields: Vec<String> = {
        let mut names: Vec<String> = r
            .changes
            .iter()
            .flat_map(|c| c.after.iter().flat_map(|a| a.keys().cloned()))
            .collect();
        names.sort();
        names.dedup();
        names
    };
    html! {<div class="card sql-result sql-preview">
        <div class="section-heading"><h2>{format!("Preview: {} {count} in {}", verb.to_lowercase(), r.collection)}</h2>{collection_link}</div>
        if !r.problems.is_empty() {<div class="error" role="alert"><strong>{"This statement cannot run:"}</strong><ul>{for r.problems.iter().map(|p| html!{<li>{p.clone()}</li>})}</ul></div>}
        <div class="table-scroll"><table class="sql-table">
            if r.kind == "delete" {
                <thead><tr><th>{"Record"}</th><th>{"id"}</th></tr></thead>
                <tbody>{for r.changes.iter().map(|c| html!{<tr><td>{c.title.clone()}</td><td class="muted">{c.id.clone()}</td></tr>})}</tbody>
            } else {
                <thead><tr><th>{"Record"}</th>{for fields.iter().map(|f| html!{<th>{f.clone()}</th>})}</tr></thead>
                <tbody>{for r.changes.iter().map(|c| html!{<tr><td>{c.title.clone()}</td>{for fields.iter().map(|f| {
                    let after = c.after.as_ref().and_then(|a| a.get(f));
                    match after {
                        Some(new) => html!{<td><span class="sql-before">{show(c.before.get(f))}</span>{" → "}<strong>{show(Some(new))}</strong></td>},
                        None => html!{<td class="muted">{"unchanged"}</td>},
                    }
                })}</tr>})}</tbody>
            }
        </table></div>
        if r.affected as usize > r.changes.len() {<p class="muted">{format!("Showing the first {} of {count}.", r.changes.len())}</p>}
        if stale {<p class="muted">{"The statement changed since this preview. Run it again to preview the new statement."}</p>}
        <div class="row">
            <button type="button" class="danger" disabled={busy || stale || r.confirmation.is_none()} onclick={apply}>{format!("{verb} {count}…")}</button>
            <button type="button" class="secondary" onclick={cancel}>{"Cancel"}</button>
        </div>
    </div>}
}

fn show(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => "empty".into(),
        Some(v) => vulse_model::display_value(v),
    }
}
