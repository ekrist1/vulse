use gloo_net::http::{Request, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use vulse_model::{
    AcceptLink, ApiKeyView, AssetView, ChangePassword, CollectionView, CreateApiKey,
    CreateCollection, CreateInvite, CreatedApiKey, CreatedInvite, DocumentView, DocumentWrite,
    InviteView, LinkInfo, LinkTokenRequest, LoginRequest, ManagedUser, Page, SetupRequest,
    SetupStatus, UpdateCollection, UpdateUser, UserView,
};
use web_sys::{File, FormData, RequestCredentials};

pub async fn me() -> Result<UserView, String> {
    get_json("/api/v1/me").await
}

pub async fn setup_status() -> Result<SetupStatus, String> {
    get_json("/api/v1/auth/setup").await
}

pub async fn setup(body: &SetupRequest) -> Result<UserView, String> {
    send_json(Request::post("/api/v1/auth/setup"), body).await
}

pub async fn inspect_link(token: &str) -> Result<LinkInfo, String> {
    send_json(
        Request::post("/api/v1/auth/links/inspect"),
        &LinkTokenRequest {
            token: token.to_string(),
        },
    )
    .await
}

pub async fn accept_link(body: &AcceptLink) -> Result<UserView, String> {
    send_json(Request::post("/api/v1/auth/links/accept"), body).await
}

pub async fn change_password(body: &ChangePassword) -> Result<(), String> {
    send_empty(Request::post("/api/v1/auth/password"), Some(body)).await
}

pub async fn logout_everywhere() -> Result<(), String> {
    send_empty::<()>(Request::post("/api/v1/auth/logout-everywhere"), None).await
}

pub async fn users() -> Result<Vec<ManagedUser>, String> {
    get_json("/api/v1/users").await
}

pub async fn update_user(id: &str, body: &UpdateUser) -> Result<ManagedUser, String> {
    send_json(Request::patch(&format!("/api/v1/users/{id}")), body).await
}

pub async fn reset_password(id: &str) -> Result<CreatedInvite, String> {
    send_json(
        Request::post(&format!("/api/v1/users/{id}/password-reset")),
        &serde_json::json!({}),
    )
    .await
}

pub async fn invites() -> Result<Vec<InviteView>, String> {
    get_json("/api/v1/invites").await
}

pub async fn create_invite(body: &CreateInvite) -> Result<CreatedInvite, String> {
    send_json(Request::post("/api/v1/invites"), body).await
}

pub async fn delete_invite(id: &str) -> Result<(), String> {
    delete(&format!("/api/v1/invites/{id}")).await
}

pub async fn login(body: &LoginRequest) -> Result<UserView, String> {
    send_json(Request::post("/api/v1/auth/login"), body).await
}

pub async fn logout() -> Result<(), String> {
    send_empty::<()>(Request::post("/api/v1/auth/logout"), None).await
}

/// Sends a request whose successful response has no body.
async fn send_empty<T: Serialize>(
    builder: gloo_net::http::RequestBuilder,
    body: Option<&T>,
) -> Result<(), String> {
    let builder = with_credentials(builder);
    let response = match body {
        Some(body) => {
            builder
                .json(body)
                .map_err(|err| err.to_string())?
                .send()
                .await
        }
        None => builder.send().await,
    }
    .map_err(|err| err.to_string())?;
    if response.ok() {
        Ok(())
    } else {
        Err(error_from(response).await)
    }
}

pub async fn collections() -> Result<Vec<CollectionView>, String> {
    let mut items = Vec::new();
    loop {
        let page: Page<CollectionView> = get_json(&format!(
            "/api/v1/collections?limit=100&offset={}",
            items.len()
        ))
        .await?;
        let count = page.items.len();
        items.extend(page.items);
        if count < 100 {
            return Ok(items);
        }
    }
}

pub async fn collection(id: &str) -> Result<CollectionView, String> {
    get_json(&format!("/api/v1/collections/{id}")).await
}

pub async fn create_collection(body: &CreateCollection) -> Result<CollectionView, String> {
    send_json(Request::post("/api/v1/collections"), body).await
}

pub async fn update_collection(
    id: &str,
    body: &UpdateCollection,
) -> Result<CollectionView, String> {
    send_json(Request::patch(&format!("/api/v1/collections/{id}")), body).await
}

pub async fn delete_collection(id: &str) -> Result<(), String> {
    delete(&format!("/api/v1/collections/{id}")).await
}

pub async fn document(collection_id: &str, document_id: &str) -> Result<DocumentView, String> {
    get_json(&format!(
        "/api/v1/collections/{collection_id}/documents/{document_id}"
    ))
    .await
}

pub async fn create_document(
    collection_id: &str,
    body: &DocumentWrite,
) -> Result<DocumentView, String> {
    send_json(
        Request::post(&format!("/api/v1/collections/{collection_id}/documents")),
        body,
    )
    .await
}

pub async fn update_document(
    collection_id: &str,
    document_id: &str,
    body: &DocumentWrite,
) -> Result<DocumentView, String> {
    send_json(
        Request::put(&format!(
            "/api/v1/collections/{collection_id}/documents/{document_id}"
        )),
        body,
    )
    .await
}

pub async fn delete_document(collection_id: &str, document_id: &str) -> Result<(), String> {
    delete(&format!(
        "/api/v1/collections/{collection_id}/documents/{document_id}"
    ))
    .await
}

pub async fn keys() -> Result<Vec<ApiKeyView>, String> {
    get_json("/api/v1/api-keys").await
}

pub async fn create_key(body: &CreateApiKey) -> Result<CreatedApiKey, String> {
    send_json(Request::post("/api/v1/api-keys"), body).await
}

pub async fn delete_key(id: &str) -> Result<(), String> {
    delete(&format!("/api/v1/api-keys/{id}")).await
}

pub async fn upload_asset(collection_id: &str, file: &File) -> Result<AssetView, String> {
    let form = FormData::new().map_err(|err| format!("{err:?}"))?;
    form.append_with_blob_and_filename("file", file, &file.name())
        .map_err(|err| format!("{err:?}"))?;
    let request = with_credentials(Request::post(&format!(
        "/api/v1/collections/{collection_id}/assets"
    )))
    .body(form)
    .map_err(|err| err.to_string())?;
    expect(request.send().await.map_err(|err| err.to_string())?).await
}

pub fn asset_content_url(collection_id: &str, asset_id: &str) -> String {
    format!("/api/v1/collections/{collection_id}/assets/{asset_id}/content")
}

async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    let response = with_credentials(Request::get(url))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    expect(response).await
}

async fn send_json<T: Serialize, R: DeserializeOwned>(
    builder: gloo_net::http::RequestBuilder,
    body: &T,
) -> Result<R, String> {
    let request = with_credentials(builder)
        .json(body)
        .map_err(|err| err.to_string())?;
    expect(request.send().await.map_err(|err| err.to_string())?).await
}

async fn delete(url: &str) -> Result<(), String> {
    let response = with_credentials(Request::delete(url))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.ok() {
        Ok(())
    } else {
        Err(error_from(response).await)
    }
}

fn with_credentials(builder: gloo_net::http::RequestBuilder) -> gloo_net::http::RequestBuilder {
    builder.credentials(RequestCredentials::Include)
}

async fn expect<T: DeserializeOwned>(response: Response) -> Result<T, String> {
    if response.ok() {
        response.json().await.map_err(|err| err.to_string())
    } else {
        Err(error_from(response).await)
    }
}

async fn error_from(response: Response) -> String {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    serde_json::from_str::<vulse_model::ApiErrorBody>(&text)
        .map(|body| match body.document_ids {
            Some(ids) if !ids.is_empty() => format!("{} ({})", body.error, ids.join(", ")),
            _ => body.error,
        })
        .unwrap_or_else(|_| {
            if text.is_empty() {
                format!("request failed ({status})")
            } else {
                text
            }
        })
}

pub async fn import_file(
    id: &str,
    file: &File,
    mapping: Option<&[Option<String>]>,
    sheet: &str,
    commit: bool,
    mode: &str,
    match_field: &str,
    has_headers: bool,
) -> Result<vulse_model::ImportResult, String> {
    let form = FormData::new().map_err(|e| format!("{e:?}"))?;
    form.append_with_blob_and_filename("file", file, &file.name())
        .map_err(|e| format!("{e:?}"))?;
    form.append_with_str("mode", mode)
        .map_err(|e| format!("{e:?}"))?;
    form.append_with_str("match_field", match_field)
        .map_err(|e| format!("{e:?}"))?;
    form.append_with_str("sheet", sheet)
        .map_err(|e| format!("{e:?}"))?;
    form.append_with_str("has_headers", if has_headers { "true" } else { "false" })
        .map_err(|e| format!("{e:?}"))?;
    form.append_with_str("commit", if commit { "true" } else { "false" })
        .map_err(|e| format!("{e:?}"))?;
    if let Some(mapping) = mapping {
        form.append_with_str(
            "mapping",
            &serde_json::to_string(mapping).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("{e:?}"))?;
    }
    let request = with_credentials(Request::post(&format!("/api/v1/collections/{id}/import")))
        .body(form)
        .map_err(|e| e.to_string())?;
    expect(request.send().await.map_err(|e| e.to_string())?).await
}

pub async fn query_records(
    id: &str,
    query: &vulse_model::RecordQuery,
) -> Result<vulse_model::RecordPage, String> {
    send_json(
        Request::post(&format!("/api/v1/collections/{id}/query")),
        query,
    )
    .await
}
pub async fn views(id: &str) -> Result<Vec<vulse_model::SavedView>, String> {
    get_json(&format!("/api/v1/collections/{id}/views")).await
}
pub async fn save_view(
    id: &str,
    view_id: Option<&str>,
    config: &vulse_model::SavedViewWrite,
) -> Result<vulse_model::SavedView, String> {
    if let Some(view) = view_id {
        send_json(
            Request::put(&format!("/api/v1/collections/{id}/views/{view}")),
            config,
        )
        .await
    } else {
        send_json(
            Request::post(&format!("/api/v1/collections/{id}/views")),
            config,
        )
        .await
    }
}
pub async fn delete_view(id: &str, view: &str) -> Result<(), String> {
    delete(&format!("/api/v1/collections/{id}/views/{view}")).await
}

pub async fn patch_record(
    id: &str,
    doc: &str,
    body: &vulse_model::DocumentPatch,
) -> Result<vulse_model::MutationResult, String> {
    send_json(
        Request::patch(&format!("/api/v1/collections/{id}/documents/{doc}")),
        body,
    )
    .await
}
pub async fn bulk_records(
    id: &str,
    body: &vulse_model::BulkWrite,
) -> Result<vulse_model::MutationResult, String> {
    send_json(
        Request::post(&format!("/api/v1/collections/{id}/bulk")),
        body,
    )
    .await
}
pub async fn undo_action(id: &str, action: &str) -> Result<vulse_model::MutationResult, String> {
    send_json(
        Request::post(&format!("/api/v1/collections/{id}/actions/{action}/undo")),
        &serde_json::json!({}),
    )
    .await
}

pub async fn revisions(
    collection: &str,
    document: &str,
    offset: u32,
) -> Result<Page<vulse_model::DocumentRevision>, String> {
    get_json(&format!(
        "/api/v1/collections/{collection}/documents/{document}/revisions?limit=20&offset={offset}"
    ))
    .await
}
pub async fn deleted_records(
    collection: &str,
    offset: u32,
) -> Result<Page<vulse_model::DocumentRevision>, String> {
    get_json(&format!(
        "/api/v1/collections/{collection}/deleted?limit=20&offset={offset}"
    ))
    .await
}
pub async fn restore_revision(
    collection: &str,
    document: &str,
    revision: &str,
    expected: Option<String>,
) -> Result<DocumentView, String> {
    send_json(
        Request::post(&format!(
            "/api/v1/collections/{collection}/documents/{document}/revisions/{revision}/restore"
        )),
        &vulse_model::RestoreRevision {
            expected_updated_at: expected,
        },
    )
    .await
}
pub async fn backlinks(
    collection: &str,
    document: &str,
    offset: u32,
) -> Result<Page<vulse_model::ReferenceView>, String> {
    get_json(&format!(
        "/api/v1/collections/{collection}/documents/{document}/references?limit=25&offset={offset}"
    ))
    .await
}

pub async fn export_records(
    collection: &str,
    write: &vulse_model::ExportRequest,
) -> Result<Vec<u8>, String> {
    let request = with_credentials(Request::post(&format!(
        "/api/v1/collections/{collection}/export"
    )))
    .json(write)
    .map_err(|e| e.to_string())?;
    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.ok() {
        return Err(error_from(response).await);
    }
    response.binary().await.map_err(|e| e.to_string())
}

pub async fn summarize(
    collection: &str,
    request: &vulse_model::SummaryRequest,
) -> Result<vulse_model::SummaryResult, String> {
    send_json(
        Request::post(&format!("/api/v1/collections/{collection}/summary")),
        request,
    )
    .await
}

pub async fn run_sql(request: &vulse_model::SqlRequest) -> Result<vulse_model::SqlResult, String> {
    send_json(Request::post("/api/v1/sql"), request).await
}
