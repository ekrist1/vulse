//! OAuth for MCP clients: discovery, registration, consent, tokens and
//! revocation.
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use super::mcp::{mcp, tool};
use super::*;

const REDIRECT: &str = "https://claude.ai/api/mcp/auth_callback";
const VERIFIER: &str = "verifier-0123456789-abcdefghijklmnopqrstuvwxyz";

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The value of one query parameter in a URL.
fn param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| {
            let mut out = Vec::new();
            let bytes = value.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    out.push(u8::from_str_radix(&value[i + 1..i + 3], 16).unwrap());
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).unwrap()
        })
    })
}

async fn get(app: &TestApp, uri: &str) -> axum::response::Response {
    app.call("GET", uri, None, None, None).await
}

async fn token(app: &TestApp, pairs: &[(&str, &str)]) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/oauth/token")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(query(pairs)))
        .unwrap();
    let response = app.app.clone().oneshot(request).await.unwrap();
    json_of::<Value>(response).await
}

async fn register_client(app: &TestApp) -> String {
    let response = app
        .call(
            "POST",
            "/oauth/register",
            None,
            None,
            Some(json!({"client_name": "Claude", "redirect_uris": [REDIRECT]})),
        )
        .await;
    let (status, client) = json_of::<Value>(response).await;
    assert_eq!(status, StatusCode::CREATED, "{client}");
    assert_eq!(client["token_endpoint_auth_method"], "none");
    client["client_id"].as_str().unwrap().to_string()
}

fn authorize_uri(client_id: &str, extra: &[(&str, &str)]) -> String {
    let challenge = challenge(VERIFIER);
    let mut pairs = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", REDIRECT),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", "xyz"),
        ("resource", "http://localhost/mcp"),
    ];
    for (name, value) in extra {
        pairs.retain(|(existing, _)| existing != name);
        if !value.is_empty() {
            pairs.push((name, value));
        }
    }
    format!("/oauth/authorize?{}", query(&pairs))
}

/// Starts a sign-in and returns the pending request id.
async fn start(app: &TestApp, client_id: &str, extra: &[(&str, &str)]) -> String {
    let response = get(app, &authorize_uri(client_id, extra)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()["location"].to_str().unwrap();
    assert!(location.starts_with("/oauth/consent?"), "{location}");
    param(location, "request").unwrap()
}

async fn approve(app: &TestApp, cookie: &str, body: Value) -> String {
    let response = app
        .call(
            "POST",
            "/api/v1/oauth/consent",
            Some(cookie),
            None,
            Some(body),
        )
        .await;
    let (status, reply) = json_of::<Value>(response).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    reply["redirect"].as_str().unwrap().to_string()
}

async fn exchange(app: &TestApp, client_id: &str, redirect: &str) -> (StatusCode, Value) {
    let code = param(redirect, "code").unwrap();
    token(
        app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", client_id),
            ("redirect_uri", REDIRECT),
            ("code_verifier", VERIFIER),
            ("resource", "http://localhost/mcp"),
        ],
    )
    .await
}

/// Runs the whole flow for a signed-in user and returns the token response.
async fn connect(app: &TestApp, cookie: &str, consent: Value) -> (String, Value) {
    let client_id = register_client(app).await;
    let request = start(app, &client_id, &[]).await;
    let mut body = json!({"request": request, "approve": true, "write": true});
    body.as_object_mut()
        .unwrap()
        .extend(consent.as_object().unwrap().clone());
    let redirect = approve(app, cookie, body).await;
    let (status, tokens) = exchange(app, &client_id, &redirect).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    (client_id, tokens)
}

async fn mcp_status(app: &TestApp, bearer: &str) -> StatusCode {
    mcp(app, Some(bearer), "tools/list", json!({})).await.0
}

#[tokio::test]
async fn oauth_metadata_points_clients_at_vulse() {
    let app = TestApp::new().await;
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
    ] {
        let response = get(&app, path).await;
        assert_eq!(response.headers()["access-control-allow-origin"], "*");
        let (status, metadata) = json_of::<Value>(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(metadata["resource"], "http://localhost/mcp");
        assert_eq!(
            metadata["authorization_servers"],
            json!(["http://localhost"])
        );
    }
    let (_, metadata) =
        json_of::<Value>(get(&app, "/.well-known/oauth-authorization-server").await).await;
    assert_eq!(metadata["issuer"], "http://localhost");
    assert_eq!(metadata["token_endpoint"], "http://localhost/oauth/token");
    assert_eq!(
        metadata["registration_endpoint"],
        "http://localhost/oauth/register"
    );
    assert_eq!(
        metadata["code_challenge_methods_supported"],
        json!(["S256"])
    );

    let response = app
        .call(
            "POST",
            "/oauth/register",
            None,
            None,
            Some(json!({"redirect_uris": ["http://evil.example/cb"]})),
        )
        .await;
    let (status, error) = json_of::<Value>(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "invalid_redirect_uri");
}

#[tokio::test]
async fn oauth_code_flow_issues_tokens_for_mcp_only() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let tasks = create_collection(
        &app,
        &cookie,
        "tasks",
        "private",
        json!({"fields":[{"name":"title","type":"string"}]}),
    )
    .await;
    let client_id = register_client(&app).await;
    let request = start(&app, &client_id, &[]).await;

    // The consent page needs a signed-in browser session.
    let info = format!("/api/v1/oauth/requests/{request}");
    assert_eq!(get(&app, &info).await.status(), StatusCode::UNAUTHORIZED);
    let (status, info) =
        json_of::<Value>(app.call("GET", &info, Some(&cookie), None, None).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        info,
        json!({"client_name": "Claude", "redirect_host": "claude.ai", "write_requested": true, "write_allowed": true})
    );

    let redirect = approve(
        &app,
        &cookie,
        json!({"request": request, "approve": true, "write": true}),
    )
    .await;
    assert!(redirect.starts_with(REDIRECT), "{redirect}");
    assert_eq!(param(&redirect, "state").unwrap(), "xyz");
    assert_eq!(param(&redirect, "iss").unwrap(), "http://localhost");

    // A wrong verifier fails, and the code cannot be tried again.
    let code = param(&redirect, "code").unwrap();
    let (status, error) = token(
        &app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &client_id),
            (
                "code_verifier",
                "wrong-verifier-0123456789-abcdefghijklmnopqrstu",
            ),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "invalid_grant");
    let (status, _) = exchange(&app, &client_id, &redirect).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (client_id, tokens) = connect(&app, &cookie, json!({})).await;
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["scope"], "records:read records:write");
    let access = tokens["access_token"].as_str().unwrap();
    assert!(access.starts_with("vat_"));

    let (created, error) = tool(
        &app,
        access,
        "create_records",
        json!({"collection": "tasks", "records": [{"title": "from claude"}]}),
    )
    .await;
    assert!(!error, "{created}");

    // OAuth tokens are only for MCP, not the REST API.
    let documents = format!("/api/v1/collections/{}/documents", tasks.id);
    assert_eq!(
        app.call("GET", &documents, None, Some(access), None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // Refresh tokens rotate; presenting an old one ends the grant.
    let old_refresh = tokens["refresh_token"].as_str().unwrap();
    let refresh = |refresh_token: String| {
        let client_id = client_id.clone();
        let app = &app;
        async move {
            token(
                app,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &refresh_token),
                    ("client_id", &client_id),
                ],
            )
            .await
        }
    };
    let (status, renewed) = refresh(old_refresh.to_string()).await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    let new_access = renewed["access_token"].as_str().unwrap();
    assert_eq!(mcp_status(&app, new_access).await, StatusCode::OK);
    let (status, error) = refresh(old_refresh.to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"], "invalid_grant");
    assert_eq!(mcp_status(&app, new_access).await, StatusCode::UNAUTHORIZED);
    let (status, _) = refresh(renewed["refresh_token"].as_str().unwrap().to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn oauth_rejects_bad_requests_without_unsafe_redirects() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let client_id = register_client(&app).await;

    // Unknown clients and unregistered redirects get a page, not a redirect.
    for uri in [
        authorize_uri("nope", &[]),
        authorize_uri(&client_id, &[("redirect_uri", "https://evil.example/cb")]),
    ] {
        let response = get(&app, &uri).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.headers().get("location").is_none());
    }

    for (extra, expected) in [
        (vec![("code_challenge_method", "plain")], "invalid_request"),
        (vec![("code_challenge", "")], "invalid_request"),
        (
            vec![("resource", "https://other.example/mcp")],
            "invalid_target",
        ),
        (vec![("scope", "admin")], "invalid_scope"),
        (
            vec![("response_type", "token")],
            "unsupported_response_type",
        ),
    ] {
        let response = get(&app, &authorize_uri(&client_id, &extra)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{extra:?}");
        let location = response.headers()["location"].to_str().unwrap();
        assert!(location.starts_with(REDIRECT), "{location}");
        assert_eq!(param(location, "error").unwrap(), expected, "{extra:?}");
        assert_eq!(param(location, "state").unwrap(), "xyz");
    }

    let request = start(&app, &client_id, &[]).await;
    let redirect = approve(&app, &cookie, json!({"request": request, "approve": false})).await;
    assert_eq!(param(&redirect, "error").unwrap(), "access_denied");
    assert!(param(&redirect, "code").is_none());
    // Each request is answered once.
    let response = app
        .call(
            "POST",
            "/api/v1/oauth/consent",
            Some(&cookie),
            None,
            Some(json!({"request": request, "approve": true})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn oauth_grants_follow_roles_scopes_and_revocation() {
    let app = TestApp::new().await;
    let (admin, _) = register(&app, "ada@example.com", "ada").await;
    let (viewer, viewer_user) = register_as(&app, "vi@example.com", "vi", Role::Viewer).await;
    let (editor, _) = register_as(&app, "ed@example.com", "ed", Role::Editor).await;
    let fields = json!({"fields":[{"name":"title","type":"string"}]});
    let posts = create_collection(&app, &admin, "posts", "private", fields.clone()).await;
    create_collection(&app, &admin, "secrets", "private", fields).await;

    // A viewer can only grant read access, whatever the app asked for.
    let client_id = register_client(&app).await;
    let request = start(&app, &client_id, &[]).await;
    let (_, info) = json_of::<Value>(
        app.call(
            "GET",
            &format!("/api/v1/oauth/requests/{request}"),
            Some(&viewer),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(info["write_allowed"], false);
    let (_, tokens) = connect(&app, &viewer, json!({})).await;
    assert_eq!(tokens["scope"], "records:read");
    let (message, error) = tool(
        &app,
        tokens["access_token"].as_str().unwrap(),
        "create_records",
        json!({"collection": "posts", "records": [{"title": "x"}]}),
    )
    .await;
    assert!(error);
    assert_eq!(message, "this action requires the editor role");

    // An editor can choose read-only, and limit the app to one collection.
    let (_, tokens) = connect(
        &app,
        &editor,
        json!({"write": false, "collection_id": posts.id}),
    )
    .await;
    assert_eq!(tokens["scope"], "records:read");
    let access = tokens["access_token"].as_str().unwrap().to_string();
    let (listed, _) = tool(&app, &access, "list_collections", json!({})).await;
    assert_eq!(listed["collections"].as_array().unwrap().len(), 1);

    // Connected apps are listed for their owner and can be disconnected.
    let (_, grants) = json_of::<Vec<OAuthGrantView>>(
        app.call("GET", "/api/v1/oauth/grants", Some(&editor), None, None)
            .await,
    )
    .await;
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].client_name, "Claude");
    assert!(!grants[0].write);
    assert_eq!(grants[0].collection_id.as_deref(), Some(posts.id.as_str()));
    assert_eq!(
        app.call(
            "DELETE",
            &format!("/api/v1/oauth/grants/{}", grants[0].id),
            Some(&viewer),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.call(
            "DELETE",
            &format!("/api/v1/oauth/grants/{}", grants[0].id),
            Some(&editor),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(mcp_status(&app, &access).await, StatusCode::UNAUTHORIZED);

    // Admins see every connected app; others cannot ask for that.
    assert_eq!(
        app.call(
            "GET",
            "/api/v1/oauth/grants?all=true",
            Some(&viewer),
            None,
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let (_, all) = json_of::<Vec<OAuthGrantView>>(
        app.call(
            "GET",
            "/api/v1/oauth/grants?all=true",
            Some(&admin),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].handle.as_deref(), Some("vi"));

    // Disabling an account disconnects its apps.
    let viewer_access = {
        let (_, tokens) = connect(&app, &viewer, json!({})).await;
        tokens["access_token"].as_str().unwrap().to_string()
    };
    assert_eq!(mcp_status(&app, &viewer_access).await, StatusCode::OK);
    let response = app
        .call(
            "PATCH",
            &format!("/api/v1/users/{}", viewer_user.id),
            Some(&admin),
            None,
            Some(json!({"disabled": true})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        mcp_status(&app, &viewer_access).await,
        StatusCode::UNAUTHORIZED
    );
    let (_, all) = json_of::<Vec<OAuthGrantView>>(
        app.call(
            "GET",
            "/api/v1/oauth/grants?all=true",
            Some(&admin),
            None,
            None,
        )
        .await,
    )
    .await;
    assert!(all.is_empty());
}

async fn set_open_registration(app: &TestApp, cookie: &str, open: bool) -> StatusCode {
    app.call(
        "PUT",
        "/api/v1/oauth/settings",
        Some(cookie),
        None,
        Some(json!({"open_registration": open})),
    )
    .await
    .status()
}

async fn clients(app: &TestApp, admin: &str) -> Vec<OAuthClientView> {
    let response = app
        .call("GET", "/api/v1/oauth/clients", Some(admin), None, None)
        .await;
    let (status, clients) = json_of::<Vec<OAuthClientView>>(response).await;
    assert_eq!(status, StatusCode::OK);
    clients
}

async fn page_text(response: axum::response::Response) -> (StatusCode, String) {
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn oauth_closed_registration_waits_for_an_admin() {
    let app = TestApp::new().await;
    let (admin, _) = register(&app, "ada@example.com", "ada").await;
    let (editor, _) = register_as(&app, "ed@example.com", "ed", Role::Editor).await;
    let (_, existing) = connect(&app, &editor, json!({})).await;

    assert_eq!(
        set_open_registration(&app, &editor, false).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        set_open_registration(&app, &admin, false).await,
        StatusCode::OK
    );
    // Apps connected earlier keep working.
    assert_eq!(
        mcp_status(&app, existing["access_token"].as_str().unwrap()).await,
        StatusCode::OK
    );

    let response = app
        .call(
            "POST",
            "/oauth/register",
            None,
            None,
            Some(json!({"client_name": "<b>Sneaky</b>", "redirect_uris": [REDIRECT]})),
        )
        .await;
    let (_, client) = json_of::<Value>(response).await;
    let client_id = client["client_id"].as_str().unwrap().to_string();
    let (status, page) = page_text(get(&app, &authorize_uri(&client_id, &[])).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(page.contains("must approve"), "{page}");
    assert!(page.contains("&lt;b&gt;Sneaky&lt;/b&gt;"), "{page}");

    let listed = clients(&app, &admin).await;
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, client_id);
    assert!(!listed[0].approved);
    assert!(listed[1].approved);
    assert_eq!(listed[1].connections, 1);
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/oauth/clients/{client_id}/approve"),
            Some(&editor),
            None,
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/oauth/clients/{client_id}/approve"),
            Some(&admin),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    start(&app, &client_id, &[]).await;

    // Clients an admin approved survive cleanup even while unused.
    let conn = app.db.connect().await.unwrap();
    conn.execute(
        "UPDATE oauth_clients SET created_at = '2000-01-01T00:00:00Z'",
        &[],
    )
    .await
    .unwrap();
    conn.execute("DELETE FROM oauth_requests", &[])
        .await
        .unwrap();
    crate::store::purge_expired(&conn, &crate::timeutil::now_secs())
        .await
        .unwrap();
    assert_eq!(clients(&app, &admin).await.len(), 2);

    // Removing a client disconnects everyone using it.
    let connected = clients(&app, &admin)
        .await
        .into_iter()
        .find(|client| client.connections == 1)
        .unwrap();
    assert_eq!(
        app.call(
            "DELETE",
            &format!("/api/v1/oauth/clients/{}", connected.id),
            Some(&admin),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        mcp_status(&app, existing["access_token"].as_str().unwrap()).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn oauth_accepts_client_metadata_documents() {
    let app = TestApp::new().await;
    let (admin, _) = register(&app, "ada@example.com", "ada").await;
    let url = "https://app.example/oauth/client.json";
    let redirect = "https://app.example/callback";
    crate::client_metadata::test_documents()
        .lock()
        .unwrap()
        .insert(
            url.into(),
            json!({"client_id": url, "client_name": "Example App", "redirect_uris": [redirect]})
                .to_string(),
        );
    let (_, metadata) =
        json_of::<Value>(get(&app, "/.well-known/oauth-authorization-server").await).await;
    assert_eq!(metadata["client_id_metadata_document_supported"], true);

    // Closed registration applies to these clients too.
    set_open_registration(&app, &admin, false).await;
    let uri = authorize_uri(url, &[("redirect_uri", redirect)]);
    let (status, page) = page_text(get(&app, &uri).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(page.contains("Example App"), "{page}");
    let listed = clients(&app, &admin).await;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].metadata_document);
    assert_eq!(listed[0].redirect_hosts, ["app.example"]);
    let encoded = encode(url);
    let response = app
        .call(
            "POST",
            &format!("/api/v1/oauth/clients/{encoded}/approve"),
            Some(&admin),
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let request = start(&app, url, &[("redirect_uri", redirect)]).await;
    let (_, info) = json_of::<Value>(
        app.call(
            "GET",
            &format!("/api/v1/oauth/requests/{request}"),
            Some(&admin),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(info["client_name"], "Example App");
    assert_eq!(info["client_domain"], "app.example");
    let back = approve(
        &app,
        &admin,
        json!({"request": request, "approve": true, "write": false}),
    )
    .await;
    assert!(back.starts_with(redirect), "{back}");
    let code = param(&back, "code").unwrap();
    let (status, tokens) = token(
        &app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", url),
            ("redirect_uri", redirect),
            ("code_verifier", VERIFIER),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    assert_eq!(
        mcp_status(&app, tokens["access_token"].as_str().unwrap()).await,
        StatusCode::OK
    );

    // A document that names another client_id is refused.
    let other = "https://other.example/client.json";
    crate::client_metadata::test_documents()
        .lock()
        .unwrap()
        .insert(
            other.into(),
            json!({"client_id": url, "redirect_uris": ["https://other.example/cb"]}).to_string(),
        );
    let uri = authorize_uri(other, &[("redirect_uri", "https://other.example/cb")]);
    let (status, page) = page_text(get(&app, &uri).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(page.contains("different client_id"), "{page}");
}
