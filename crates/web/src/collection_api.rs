use crate::app::Route;
use serde_json::{Value, json};
use vulse_model::{ArrayItems, CollectionView, DocumentView, FieldType, Visibility};
use wasm_bindgen::prelude::*;
use yew::prelude::*;
use yew_router::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "VulseEditor"], js_name = copy)]
    async fn copy_text(text: &str) -> Result<JsValue, JsValue>;
}

#[derive(Properties, PartialEq)]
struct CodeProps {
    pub title: String,
    pub text: String,
}
#[function_component(CodeExample)]
fn code_example(props: &CodeProps) -> Html {
    let status = use_state(String::new);
    {
        let status = status.clone();
        use_effect_with(props.text.clone(), move |_| status.set(String::new()));
    }
    let copy = {
        let text = props.text.clone();
        let status = status.clone();
        Callback::from(move |_| {
            let text = text.clone();
            let status = status.clone();
            wasm_bindgen_futures::spawn_local(async move {
                status.set(if copy_text(&text).await.is_ok() {
                    "Copied".into()
                } else {
                    "Select the code and copy it manually.".into()
                });
            });
        })
    };
    html! {<div class="api-code"><div class="api-code-heading"><span>{props.title.clone()}</span><div class="row"><span role="status">{(*status).clone()}</span><button type="button" class="secondary" aria-label={format!("Copy {}",props.title)} onclick={copy}>{"Copy"}</button></div></div><pre><code>{props.text.clone()}</code></pre></div>}
}
/// Minimum role for an endpoint. Query, export and summary are reads sent as POST.
fn required_role(method: &str, url: &str, schema: bool) -> &'static str {
    if schema {
        "Admin, all collections"
    } else if method == "GET"
        || url.ends_with("/query")
        || url.ends_with("/export")
        || url.ends_with("/summary")
    {
        "Viewer"
    } else if method.starts_with("GET") {
        "Viewer reads, editor writes"
    } else {
        "Editor"
    }
}

#[derive(Properties, PartialEq)]
pub struct CollectionApiProps {
    pub collection: CollectionView,
}
fn sample_body(collection: &CollectionView) -> Value {
    let mut body = serde_json::Map::new();
    for field in &collection.blueprint.fields {
        let value = match field.kind {
            FieldType::Calculated => continue,
            FieldType::String => json!("Example"),
            FieldType::Number => json!(42),
            FieldType::Boolean => json!(true),
            FieldType::Date => json!("2026-10-11"),
            FieldType::Json => json!({"example":true}),
            FieldType::PortableText => {
                json!([{"_type":"block","_key":"example","style":"normal","markDefs":[],"children":[{"_type":"span","_key":"span","text":"Example content","marks":[]}]}])
            }
            FieldType::Asset | FieldType::Reference => {
                json!("550e8400-e29b-41d4-a716-446655440000")
            }
            FieldType::Array => match field.items {
                Some(ArrayItems::Number) => json!([42]),
                Some(ArrayItems::Asset | ArrayItems::Reference) => {
                    json!(["550e8400-e29b-41d4-a716-446655440000"])
                }
                _ => json!(["Example"]),
            },
        };
        body.insert(field.name.clone(), value);
    }
    Value::Object(body)
}
fn pretty(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn curl(method: &str, url: &str, payload: Option<&Value>, auth: bool) -> String {
    let mut text = format!(
        "curl{} {}",
        if method == "GET" {
            String::new()
        } else {
            format!(" -X {method}")
        },
        shell_quote(url)
    );
    if auth {
        text.push_str(" \\\n  -H \"Authorization: Bearer $VULSE_API_KEY\"");
    }
    if let Some(payload) = payload {
        text.push_str(&format!(
            " \\\n  -H 'Content-Type: application/json' \\\n  --data {}",
            shell_quote(&pretty(payload))
        ));
    }
    text
}
#[function_component(CollectionApi)]
pub fn collection_api(props: &CollectionApiProps) -> Html {
    let collection = &props.collection;
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    let base = format!("{origin}/api/v1/collections/{}", collection.id);
    let documents = format!("{base}/documents");
    let document = format!("{documents}/DOCUMENT_ID");
    let body = sample_body(collection);
    let (computed, errors) = vulse_model::calculate_document(&collection.blueprint, &body);
    let response = DocumentView {
        id: "550e8400-e29b-41d4-a716-446655440000".into(),
        body: computed,
        created_at: "2026-10-11T12:00:00Z".into(),
        updated_at: "2026-10-11T12:00:00Z".into(),
        calculation_errors: errors,
    };
    let public = format!("{origin}/api/v1/public/{}", collection.name);
    let endpoints = [
        ("GET", base.clone(), "Read collection schema", false),
        ("PATCH", base.clone(), "Change schema or settings", true),
        (
            "DELETE",
            base.clone(),
            "Delete collection and contents",
            true,
        ),
        (
            "GET",
            documents.clone(),
            "List records · limit / offset",
            false,
        ),
        ("POST", documents.clone(), "Create record", false),
        ("GET", document.clone(), "Read record", false),
        ("PUT", document.clone(), "Replace record body", false),
        (
            "DELETE",
            document.clone(),
            "Delete record · recover via history",
            false,
        ),
        ("PATCH", document.clone(), "Update specified fields", false),
        (
            "POST",
            format!("{base}/query"),
            "Search, filter, sort and page · includes total",
            false,
        ),
        (
            "GET / POST",
            format!("{base}/views"),
            "List or create saved views",
            false,
        ),
        (
            "PUT / DELETE",
            format!("{base}/views/VIEW_ID"),
            "Update or delete a saved view",
            false,
        ),
        (
            "POST",
            format!("{base}/bulk"),
            "Update, duplicate or delete 1–100 records",
            false,
        ),
        (
            "POST",
            format!("{base}/actions/ACTION_ID/undo"),
            "Undo a batch within 24 hours",
            false,
        ),
        (
            "GET",
            format!("{document}/revisions"),
            "Read revision history · limit / offset",
            false,
        ),
        (
            "POST",
            format!("{document}/revisions/REVISION_ID/restore"),
            "Restore a saved or deleted record",
            false,
        ),
        (
            "GET",
            format!("{base}/deleted"),
            "List deleted records · limit / offset",
            false,
        ),
        (
            "GET",
            format!("{document}/references"),
            "Read incoming references · limit / offset",
            false,
        ),
        (
            "POST",
            format!("{base}/export"),
            "Download a filtered view or selection",
            false,
        ),
        (
            "POST",
            format!("{base}/summary"),
            "Group records; sum, average, min, max, count and formulas",
            false,
        ),
        (
            "GET",
            format!("{base}/views/VIEW_ID/summary"),
            "Run a saved view's summary",
            false,
        ),
        (
            "GET",
            format!("{base}/assets"),
            "List assets · limit / offset",
            false,
        ),
        (
            "POST",
            format!("{base}/assets"),
            "Upload asset · multipart file",
            false,
        ),
        (
            "GET",
            format!("{base}/assets/ASSET_ID"),
            "Read asset metadata",
            false,
        ),
        (
            "GET",
            format!("{base}/assets/ASSET_ID/content"),
            "Download asset",
            false,
        ),
        (
            "DELETE",
            format!("{base}/assets/ASSET_ID"),
            "Delete asset",
            false,
        ),
        (
            "POST",
            format!("{base}/import"),
            "Preview or commit CSV / XLSX import",
            false,
        ),
    ];
    let import = format!(
        "curl -X POST {} \\\n  -H \"Authorization: Bearer $VULSE_API_KEY\" \\\n  -F 'file=@data.csv' \\\n  -F 'commit=false'",
        shell_quote(&format!("{base}/import"))
    );
    html! {<div class="api-panel">
      <div class="section-heading"><div><h2>{"Build with your collection"}</h2><p class="muted">{"Live endpoint details and examples based on your saved fields."}</p></div><span class="badge">{"REST · JSON · v1"}</span></div>
      <section class="card api-auth"><h3>{"Authenticate your requests"}</h3><p>{"Create an API key, then send it in the Authorization header. A key has a role no higher than yours: viewer keys read, editor keys also write records, assets and imports, and admin keys also change schemas. A key scoped to this collection cannot reach others, and schema changes need an admin key for all collections."}</p><Link<Route> classes="button secondary" to={Route::Keys}>{"Manage API keys →"}</Link<Route>><CodeExample title="Authorization header" text={"Authorization: Bearer YOUR_API_KEY"}/><p class="muted">{"The app also accepts your signed-in session cookie on same-origin requests. Keep API keys on your server; public reads are available without a key when a collection is public."}</p></section>
      <section class="card"><h3>{"Endpoints"}</h3><p class="muted">{"Replace DOCUMENT_ID and ASSET_ID with returned IDs. All endpoints below require authentication."}</p><div class="table-scroll"><table class="api-endpoints"><thead><tr><th>{"Method"}</th><th>{"Path"}</th><th>{"Purpose"}</th><th>{"Minimum role"}</th></tr></thead><tbody>{for endpoints.iter().map(|(method,url,description,account)|html!{<tr><td><span class={format!("api-method method-{}",method.to_lowercase())}>{*method}</span></td><td><code>{url.strip_prefix(&origin).unwrap_or(url)}</code></td><td>{*description}</td><td>{required_role(method,url,*account)}</td></tr>})}</tbody></table></div></section>
      <section class="card"><h3>{"Read records"}</h3><p>{"Lists return items, limit, and offset. The default limit is 50; choose 1–100. Increase offset until a page returns fewer items than limit. There is no total count in the response."}</p>
        <CodeExample title="List request" text={curl("GET",&format!("{documents}?limit=50&offset=0"),None,true)}/>
        <CodeExample title="Example list response · 200" text={pretty(&json!({"items":[response.clone()],"limit":50,"offset":0}))}/>
        <details><summary>{"Read a single record"}</summary><CodeExample title="Read request" text={curl("GET",&document,None,true)}/><CodeExample title="Example record response · 200" text={pretty(&response)}/></details>
      </section>
      <section class="card"><h3>{"Create and update records"}</h3><p>{"Wrap field values in a body object. Calculated fields are generated by the server and omitted from write examples. Responses include their current results; calculation failures appear in calculation_errors, with null as the field value."}</p>
        if collection.blueprint.fields.iter().any(|f| matches!(f.kind,FieldType::Asset|FieldType::Reference)|| matches!(f.items,Some(ArrayItems::Asset|ArrayItems::Reference))){<p class="api-note">{"The example UUIDs are placeholders. Upload assets first, and use existing record IDs from the reference field’s target collection."}</p>}
        <CodeExample title="Create request" text={curl("POST",&documents,Some(&json!({"body":body.clone()})),true)}/><CodeExample title="Example create response · 201" text={pretty(&response)}/>
        <details><summary>{"Update a record"}</summary><p>{"PUT replaces the entire body. Include every field you want to keep; omitted optional values are removed. Send all required fields."}</p><CodeExample title="Update request" text={curl("PUT",&document,Some(&json!({"body":body.clone(),"expected_updated_at":response.updated_at})),true)}/><p class="muted">{"Returns the updated record with status 200, using the same shape as the create response."}</p></details>
        <details><summary>{"Delete a record"}</summary><CodeExample title="Delete request" text={curl("DELETE",&document,None,true)}/><p class="muted">{"Success returns 204 with an empty body."}</p></details>
      </section>
      <section class="card"><h3>{"Search and saved views"}</h3><p>{"POST query accepts search, filters, sort_field, descending, group_field, limit (1–100), and offset. Its response adds total to the paginated items. Filter fields use their API names. Operators: contains, eq, neq, gt, gte, lt, lte, empty, not_empty. Numeric comparisons require numeric values."}</p>
        <CodeExample title="Search request" text={curl("POST",&format!("{base}/query"),Some(&json!({"search":"Example","filters":[],"sort_field":"updated_at","descending":true,"limit":25,"offset":0})),true)}/>
        <CodeExample title="Save view request" text={curl("POST",&format!("{base}/views"),Some(&json!({"name":"Recent examples","query":{"search":"Example","sort_field":"updated_at","descending":true},"columns":collection.blueprint.fields.iter().map(|f|&f.name).collect::<Vec<_>>()})),true)}/>
      </section>
      <section class="card"><h3>{"Safe edits, bulk actions and recovery"}</h3><p>{"Send expected_updated_at from the record you read with PUT or PATCH to detect stale edits. Collection schema PATCH accepts the collection’s timestamp too. PATCH merges only supplied fields; calculated fields are read-only."}</p>
        <CodeExample title="Partial update request" text={curl("PATCH",&document,Some(&json!({"patch":body.clone(),"expected_updated_at":response.updated_at})),true)}/>
        <CodeExample title="Bulk update request" text={curl("POST",&format!("{base}/bulk"),Some(&json!({"action":"update","ids":["DOCUMENT_ID"],"patch":body.clone(),"expected":{"DOCUMENT_ID":response.updated_at}})),true)}/>
        <p>{"Bulk actions and PATCH return documents, affected, and undo_id. Other bulk actions are duplicate and delete. Undo checks that affected records have not changed, and lasts 24 hours. Delete returns 204 for a single record; use bulk to receive an undo token. Persistent revision history remains available after deletion."}</p>
        <CodeExample title="Restore revision request" text={curl("POST",&format!("{document}/revisions/REVISION_ID/restore"),Some(&json!({"expected_updated_at":response.updated_at})),true)}/>
        <p>{"Existing records require their current timestamp when restoring. Deleted records use an empty object. Restores validate the body against current fields, references and assets. Collection renames retarget incoming schemas atomically. Deleting referenced records or assets returns 409 with the affected document_ids; remove incoming references first. Referencing schema fields must be removed before deleting a collection."}</p>
      </section>
      <section class="card"><h3>{"Export records"}</h3><p>{"POST export returns a CSV or JSON download. Pass format, columns (API field names), and query for all matching rows across pages; pagination is ignored. Alternatively pass ids for up to 100 selected records. Export supports up to 10,000 records and 25 MB, with an explicit error if either limit is exceeded. CSV uses technical field headers and serializes structured values as JSON. Spreadsheet formula-like strings are prefixed with an apostrophe; JSON preserves exact values and record IDs."}</p>
        <CodeExample title="Export view request" text={format!("{} --output collection.json",curl("POST",&format!("{base}/export"),Some(&json!({"format":"json","columns":collection.blueprint.fields.iter().map(|f|&f.name).collect::<Vec<_>>(),"query":{"search":"Example"}})),true))}/>
      </section>
      <section class="card"><h3>{"Upload and import"}</h3><details><summary>{"Upload an asset"}</summary><p>{"Send one multipart file, up to 25 MB. Store its returned id in an asset field or a Portable Text image reference."}</p><CodeExample title="Upload request" text={format!("curl -X POST {} \\\n  -H \"Authorization: Bearer $VULSE_API_KEY\" \\\n  -F 'file=@image.png'",shell_quote(&format!("{base}/assets")))}/><CodeExample title="Example asset response · 201" text={pretty(&json!({"id":"550e8400-e29b-41d4-a716-446655440000","filename":"image.png","content_type":"image/png","byte_size":1024,"created_at":"2026-10-11T12:00:00Z"}))}/></details>
        <details><summary>{"Import CSV or Excel"}</summary><p>{"Send CSV or XLSX, up to 50 MB and 50,000 data rows. The first row contains column names. Preview with commit=false (the default), then send the same file with commit=true. A commit with validation errors writes zero records."}</p><CodeExample title="Import preview request" text={import}/><p>{"Optional multipart fields: sheet selects an XLSX worksheet; mapping is a JSON array of destination field names in column order, using null to skip a column. Without mapping, matching header names are selected automatically. Set mode=upsert and match_field to a string or number field to update existing records by a unique value; otherwise mode=append adds new records. Blank cells preserve existing values during updates, and unmapped fields are kept. Duplicate keys in the file or collection stop the entire import. Preview and commit both report created, updated, unchanged, and preview_actions; imported counts actual writes. Commit revalidates against current data."}</p><CodeExample title="Example import response · 200" text={pretty(&json!({"headers":["source_column"],"sheets":[],"sheet":"","mapping":[null],"row_count":1,"preview":[],"errors":[{"row":2,"field":null,"message":"Example validation error"}],"imported":0}))}/><p class="muted">{"Inspect errors even when the HTTP status is 200. Errors identify the source row, field when available, and message. Preview includes at most 10 records."}</p></details>
      </section>
      <section class="card"><h3>{"Public read API"}</h3>
        if collection.visibility==Visibility::Public {<p>{"This collection is public. Anyone can read its records and asset content without authentication. Public endpoints support cross-origin GET requests."}</p><CodeExample title="Public list request" text={curl("GET",&format!("{public}?limit=50&offset=0"),None,false)}/><p>{"The response has the same paginated shape as the authenticated list."}</p><CodeExample title="Public record and asset URLs" text={format!("GET {public}/DOCUMENT_ID\nGET {public}/assets/ASSET_ID")}/>}
        else {<p>{"This collection is private. Public endpoints return 404. Change visibility in Settings if you want unauthenticated read access."}</p>}
      </section>
      <section class="card"><h3>{"Validation and errors"}</h3><p>{"Send numbers and booleans as JSON values, dates as \"YYYY-MM-DD\" strings, references and assets as UUID strings, and rich text as Portable Text arrays. The editor uses Tiptap; the stored API format remains Portable Text."}</p><div class="table-scroll"><table><tbody>{for [("400","Invalid JSON, schema, fields, references, or query parameters"),("401","Missing or invalid credentials"),("403","Your role or the API key's role or scope does not permit this action, or a browser sent a cross-site request"),("404","Collection, record, or asset not found or inaccessible"),("409","Stale edit, invalid schema change, blocked deletion or undo conflict"),("413","Request exceeds the upload or JSON size limit")].iter().map(|(code,description)|html!{<tr><th>{*code}</th><td>{*description}</td></tr>})}</tbody></table></div><CodeExample title="Example error response" text={pretty(&json!({"error":"Example validation message"}))}/><p class="muted">{"JSON writes are limited to 1 MB. A schema conflict can also include document_ids to identify incompatible records. Responses above are illustrative, generated from your current schema."}</p></section>
    </div>}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn write_example_omits_calculated_fields() {
        let collection:CollectionView=serde_json::from_value(json!({"id":"id","name":"test","visibility":"private","created_at":"","updated_at":"","blueprint":{"fields":[{"name":"price","type":"number"},{"name":"total","type":"calculated","formula":"=[price]*2"}]}})).unwrap();
        assert_eq!(sample_body(&collection), json!({"price":42}));
    }
    #[test]
    fn curl_quotes_apostrophes_and_wraps_body() {
        let request = curl(
            "PUT",
            "https://example.test/doc",
            Some(&json!({"body":{"title":"it's fine"}})),
            true,
        );
        assert!(request.contains("'\\''"));
        assert!(request.contains("Authorization: Bearer $VULSE_API_KEY"));
        assert!(request.contains("\"body\""));
    }
}
