use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use vulse_model::{
    ApiErrorBody, ApiKeyView, AssetView, CollectionView, CreatedApiKey, CreatedInvite,
    DocumentView, ManagedUser, OAuthClientView, OAuthGrantView, Page, Role, UserView,
};

use crate::app;

#[path = "tests/bench.rs"]
mod bench;
#[path = "tests/beta.rs"]
mod beta;
#[path = "tests/mcp.rs"]
mod mcp;
#[path = "tests/oauth.rs"]
mod oauth;

struct TestApp {
    app: axum::Router,
    asset_dir: std::path::PathBuf,
    db: crate::db::Db,
    /// Session of the first admin, created on the first `register` call.
    admin: tokio::sync::Mutex<Option<String>>,
    _dir: tempfile::TempDir,
}

impl TestApp {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_state(dir.path().to_path_buf()).await.unwrap();
        Self::with_state(state, dir)
    }

    fn with_state(state: crate::state::AppState, dir: tempfile::TempDir) -> Self {
        Self {
            asset_dir: dir.path().join("assets"),
            db: state.db.clone(),
            app: app(state),
            admin: tokio::sync::Mutex::new(None),
            _dir: dir,
        }
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        bearer: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        self.call_with(method, uri, cookie, bearer, body, &[]).await
    }

    async fn call_with(
        &self,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        bearer: Option<&str>,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", format!("vulse_session={cookie}"));
        }
        if let Some(bearer) = bearer {
            builder = builder.header("authorization", format!("Bearer {bearer}"));
        }
        let payload = body.map(|value| value.to_string()).unwrap_or_default();
        self.app
            .clone()
            .oneshot(builder.body(Body::from(payload)).unwrap())
            .await
            .unwrap()
    }
}

async fn json_of<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
) -> (StatusCode, T) {
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|err| panic!("{err}: {}", String::from_utf8_lossy(&bytes)));
    (status, value)
}

fn session_of(response: &axum::response::Response) -> String {
    let raw = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(raw.contains("HttpOnly"));
    assert!(raw.contains("SameSite=Lax"));
    assert!(raw.contains("Path=/"));
    raw.split(';')
        .next()
        .unwrap()
        .trim_start_matches("vulse_session=")
        .to_string()
}

/// The first account becomes the admin through setup. Later accounts are
/// invited as admins, so each test can manage its own collections.
async fn register(app: &TestApp, email: &str, handle: &str) -> (String, UserView) {
    let mut admin = app.admin.lock().await;
    if admin.is_none() {
        let response = app
            .call(
                "POST",
                "/api/v1/auth/setup",
                None,
                None,
                Some(json!({"token": crate::TEST_SETUP_TOKEN, "email": email, "handle": handle, "password": "password123"})),
            )
            .await;
        let cookie = session_of(&response);
        let (status, user) = json_of::<UserView>(response).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(user.role, Role::Admin);
        *admin = Some(cookie.clone());
        return (cookie, user);
    }
    let admin = admin.clone().unwrap();
    invite_and_accept(app, &admin, email, handle, Role::Admin).await
}

async fn register_as(app: &TestApp, email: &str, handle: &str, role: Role) -> (String, UserView) {
    let admin = app
        .admin
        .lock()
        .await
        .clone()
        .expect("register an admin first");
    invite_and_accept(app, &admin, email, handle, role).await
}

async fn invite(app: &TestApp, admin: &str, email: &str, role: Role) -> CreatedInvite {
    let (status, invite) = json_of::<CreatedInvite>(
        app.call(
            "POST",
            "/api/v1/invites",
            Some(admin),
            None,
            Some(json!({"email": email, "role": role})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(invite.path, format!("/accept#token={}", invite.token));
    invite
}

async fn accept(
    app: &TestApp,
    token: &str,
    handle: Option<&str>,
    password: &str,
) -> axum::response::Response {
    app.call(
        "POST",
        "/api/v1/auth/links/accept",
        None,
        None,
        Some(json!({"token": token, "handle": handle, "password": password})),
    )
    .await
}

async fn invite_and_accept(
    app: &TestApp,
    admin: &str,
    email: &str,
    handle: &str,
    role: Role,
) -> (String, UserView) {
    let invite = invite(app, admin, email, role).await;
    let response = accept(app, &invite.token, Some(handle), "password123").await;
    let cookie = session_of(&response);
    let (status, user) = json_of::<UserView>(response).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user.role, role);
    (cookie, user)
}

async fn login(app: &TestApp, email: &str, password: &str) -> axum::response::Response {
    app.call(
        "POST",
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({"email": email, "password": password})),
    )
    .await
}

#[tokio::test]
async fn session_and_api_keys() {
    let app = TestApp::new().await;
    let (cookie, user) = register(&app, "ada@example.com", "ada").await;
    let (status, me) = json_of::<UserView>(
        app.call("GET", "/api/v1/me", Some(&cookie), None, None)
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me.handle, user.handle);

    assert_eq!(
        app.call("POST", "/api/v1/auth/logout", Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.call("GET", "/api/v1/me", Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let login = app
        .call(
            "POST",
            "/api/v1/auth/login",
            None,
            None,
            Some(json!({"email": "ada@example.com", "password": "wrong-password"})),
        )
        .await;
    assert_eq!(login.status(), StatusCode::UNAUTHORIZED);
    let login = app
        .call(
            "POST",
            "/api/v1/auth/login",
            None,
            None,
            Some(json!({"email": "ada@example.com", "password": "password123"})),
        )
        .await;
    let cookie = session_of(&login);
    assert_eq!(login.status(), StatusCode::OK);

    let duplicate = app
        .call(
            "POST",
            "/api/v1/invites",
            Some(&cookie),
            None,
            Some(json!({"email": "ada@example.com", "role": "editor"})),
        )
        .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    let created = app
        .call(
            "POST",
            "/api/v1/api-keys",
            Some(&cookie),
            None,
            Some(json!({"name": "site"})),
        )
        .await;
    let (status, key) = json_of::<CreatedApiKey>(created).await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(key.secret.starts_with(&key.key.prefix));
    let (status, me) = json_of::<UserView>(
        app.call("GET", "/api/v1/me", None, Some(&key.secret), None)
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me.id, user.id);

    let (status, keys) = json_of::<Vec<ApiKeyView>>(
        app.call("GET", "/api/v1/api-keys", Some(&cookie), None, None)
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys.len(), 1);
    assert!(!format!("{keys:?}").contains(&key.secret));
}

#[tokio::test]
async fn collections_visibility_and_blueprint_rules() {
    let app = TestApp::new().await;
    let (cookie, _user) = register(&app, "ada@example.com", "ada").await;
    let authors = create_collection(
        &app,
        &cookie,
        "authors",
        "private",
        json!({"fields": [{"name": "name", "type": "string", "required": true}]}),
    )
    .await;
    let author = create_document(&app, &cookie, &authors.id, json!({"name": "Ada"})).await;
    let posts = create_collection(
        &app,
        &cookie,
        "posts",
        "public",
        json!({
            "fields": [
                {"name": "title", "type": "string", "required": true},
                {"name": "body", "type": "portable_text"},
                {"name": "author", "type": "reference", "collection": "authors"}
            ]
        }),
    )
    .await;
    let invalid = app
        .call(
            "POST",
            &format!("/api/v1/collections/{}/documents", posts.id),
            Some(&cookie),
            None,
            Some(json!({"body": {"title": "Hello", "author": "missing"}})),
        )
        .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    let post = create_document(
        &app,
        &cookie,
        &posts.id,
        json!({"title": "Hello", "author": author.id}),
    )
    .await;

    let public_doc = app
        .call(
            "GET",
            &format!("/api/v1/public/posts/{}", post.id),
            None,
            None,
            None,
        )
        .await;
    assert_eq!(
        public_doc
            .headers()
            .get("access-control-allow-origin")
            .unwrap(),
        "*"
    );
    let (status, seen) = json_of::<DocumentView>(public_doc).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen.body["title"], "Hello");

    let secrets = create_collection(
        &app,
        &cookie,
        "secrets",
        "private",
        json!({"fields": [{"name": "note", "type": "string", "required": true}]}),
    )
    .await;
    let _secret = create_document(
        &app,
        &cookie,
        &secrets.id,
        json!({"note": "top-secret-phrase"}),
    )
    .await;
    let hidden = app
        .call("GET", "/api/v1/public/secrets", None, None, None)
        .await;
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let bytes = hidden.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&bytes).contains("top-secret-phrase"));
    assert_eq!(
        app.call("GET", "/api/v1/public/missing", None, None, None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // Private collections are shared with every workspace member.
    let (viewer, _) = register_as(&app, "grace@example.com", "grace", Role::Viewer).await;
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}", secrets.id),
            Some(&viewer),
            None,
            None
        )
        .await
        .status(),
        StatusCode::OK
    );

    let scoped = app
        .call(
            "POST",
            "/api/v1/api-keys",
            Some(&cookie),
            None,
            Some(json!({"name": "posts-only", "collection_id": posts.id})),
        )
        .await;
    let (_, scoped) = json_of::<CreatedApiKey>(scoped).await;
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}/documents/{}", posts.id, post.id),
            None,
            Some(&scoped.secret),
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}", secrets.id),
            None,
            Some(&scoped.secret),
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/collections",
            None,
            Some(&scoped.secret),
            Some(json!({"name": "nope", "visibility": "public", "blueprint": {"fields": []}}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let broken = app
        .call(
            "PATCH",
            &format!("/api/v1/collections/{}", posts.id),
            Some(&cookie),
            None,
            Some(json!({"blueprint": {"fields": [{"name": "name", "type": "string", "required": true}]}})),
        )
        .await;
    let (status, err) = json_of::<ApiErrorBody>(broken).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(err.document_ids.unwrap(), vec![post.id.clone()]);

    let extended = app
        .call(
            "PATCH",
            &format!("/api/v1/collections/{}", posts.id),
            Some(&cookie),
            None,
            Some(json!({
                "blueprint": {
                    "fields": [
                        {"name": "title", "type": "string", "required": true},
                        {"name": "subtitle", "type": "string"},
                        {"name": "body", "type": "portable_text"},
                        {"name": "author", "type": "reference", "collection": "authors"}
                    ]
                }
            })),
        )
        .await;
    assert_eq!(extended.status(), StatusCode::OK);
}

#[tokio::test]
async fn patches_bulk_and_undo_are_atomic_and_conflict_aware() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "bulk@example.com", "bulk").await;
    let collection=create_collection(&app,&cookie,"products","private",json!({"fields":[{"name":"sku","type":"string","required":true},{"name":"price","type":"number"},{"name":"total","type":"calculated","formula":"=[price]*2"}]})).await;
    let first = create_document(&app, &cookie, &collection.id, json!({"sku":"A","price":2})).await;
    let second = create_document(&app, &cookie, &collection.id, json!({"sku":"B","price":3})).await;
    let endpoint = format!(
        "/api/v1/collections/{}/documents/{}",
        collection.id, first.id
    );
    let bulk = format!("/api/v1/collections/{}/bulk", collection.id);
    let (status, patched) = json_of::<vulse_model::MutationResult>(
        app.call(
            "PATCH",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"patch":{"price":5},"expected_updated_at":first.updated_at})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched.documents[0].body["total"].as_f64(), Some(10.0));
    assert_eq!(
        app.call(
            "PATCH",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"patch":{"price":99},"expected_updated_at":first.updated_at}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "PATCH",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"patch":{"total":1}}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call(
            "POST",
            &bulk,
            Some(&cookie),
            None,
            Some(json!({"action":"update","ids":[first.id,second.id],"patch":{"sku":null}}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let (_, seen) =
        json_of::<DocumentView>(app.call("GET", &endpoint, Some(&cookie), None, None).await).await;
    assert_eq!(seen.body["price"], 5);
    let undo = |id: &str| format!("/api/v1/collections/{}/actions/{id}/undo", collection.id);
    assert_eq!(
        app.call("POST", &undo(&patched.undo_id), Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::OK
    );
    let (_, seen) =
        json_of::<DocumentView>(app.call("GET", &endpoint, Some(&cookie), None, None).await).await;
    assert_eq!(seen.body["price"], 2);
    let (_, deleted) = json_of::<vulse_model::MutationResult>(
        app.call(
            "POST",
            &bulk,
            Some(&cookie),
            None,
            Some(json!({"action":"delete","ids":[first.id,second.id]})),
        )
        .await,
    )
    .await;
    assert_eq!(deleted.affected, 2);
    assert_eq!(
        app.call("GET", &endpoint, Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let (_, restored) = json_of::<vulse_model::MutationResult>(
        app.call("POST", &undo(&deleted.undo_id), Some(&cookie), None, None)
            .await,
    )
    .await;
    assert_eq!(restored.documents.len(), 2);
    let (_, duplicates) = json_of::<vulse_model::MutationResult>(
        app.call(
            "POST",
            &bulk,
            Some(&cookie),
            None,
            Some(json!({"action":"duplicate","ids":[first.id,second.id]})),
        )
        .await,
    )
    .await;
    assert_eq!(duplicates.documents.len(), 2);
    assert_ne!(duplicates.documents[0].id, first.id);
    assert_eq!(
        app.call(
            "POST",
            &undo(&duplicates.undo_id),
            Some(&cookie),
            None,
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
    let (_, patched) = json_of::<vulse_model::MutationResult>(
        app.call(
            "PATCH",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"patch":{"price":5}})),
        )
        .await,
    )
    .await;
    app.call(
        "PATCH",
        &endpoint,
        Some(&cookie),
        None,
        Some(json!({"patch":{"price":6}})),
    )
    .await;
    assert_eq!(
        app.call("POST", &undo(&patched.undo_id), Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let (viewer, _) = register_as(&app, "other-bulk@example.com", "otherbulk", Role::Viewer).await;
    assert_eq!(
        app.call(
            "POST",
            &bulk,
            Some(&viewer),
            None,
            Some(json!({"action":"delete","ids":[first.id]}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn record_tables_sort_empty_values_last_and_follow_changes() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "rows@example.com", "rows").await;
    let c = create_collection(
        &app,
        &cookie,
        "scores",
        "private",
        json!({"fields":[{"name":"title","type":"string","required":true},{"name":"score","type":"number"},{"name":"double","type":"calculated","formula":"=[score]*2"}]}),
    )
    .await;
    for (title, score) in [
        ("c", json!(3)),
        ("a", json!(1)),
        ("x", Value::Null),
        ("b", json!(2)),
        ("y", Value::Null),
    ] {
        let mut body = json!({"title": title});
        if !score.is_null() {
            body["score"] = score;
        }
        create_document(&app, &cookie, &c.id, body).await;
    }
    let endpoint = format!("/api/v1/collections/{}/query", c.id);
    let titles = async |query: Value| {
        let (status, page) = json_of::<vulse_model::RecordPage>(
            app.call("POST", &endpoint, Some(&cookie), None, Some(query.clone()))
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{query}");
        let titles: Vec<String> = page
            .items
            .iter()
            .map(|d| d.body["title"].as_str().unwrap().to_string())
            .collect();
        (page.total, titles.join(""))
    };
    // Pages cross from records with a value to records without one.
    let ascending = |offset: u32, limit: u32| json!({"sort_field":"score","descending":false,"offset":offset,"limit":limit});
    assert_eq!(titles(ascending(0, 2)).await, (5, "ab".into()));
    let (_, page) = titles(ascending(2, 2)).await;
    assert!(page == "cx" || page == "cy", "{page}");
    let (_, rest) = titles(ascending(3, 2)).await;
    assert_eq!(rest.len(), 2);
    assert!(rest.contains('x') && rest.contains('y'));
    assert_eq!(titles(ascending(4, 2)).await.1.len(), 1);
    assert_eq!(titles(ascending(5, 2)).await.1, "");
    let (_, descending) = titles(json!({"sort_field":"score","descending":true})).await;
    assert!(descending.starts_with("cba"), "{descending}");
    let (_, calculated) = titles(json!({"sort_field":"double","descending":false})).await;
    // An empty score counts as 0 in the formula.
    assert!(calculated.ends_with("abc"), "{calculated}");
    let (_, grouped) =
        titles(json!({"group_field":"score","sort_field":"title","descending":false})).await;
    assert_eq!(grouped.len(), 5);
    // Negated filters include records without a value.
    assert_eq!(
        titles(json!({"filters":[{"field":"score","op":"neq","value":2}]}))
            .await
            .0,
        4
    );
    assert_eq!(
        titles(json!({"filters":[{"field":"score","op":"empty"}]}))
            .await
            .0,
        2
    );
    assert_eq!(
        titles(json!({"filters":[{"field":"double","op":"gte","value":4}]}))
            .await
            .0,
        2
    );
    // Filters joined by OR form a group that the other filters narrow down.
    assert_eq!(
        titles(json!({"filters":[{"field":"score","op":"eq","value":1},{"field":"score","op":"eq","value":3,"or":true}],"sort_field":"title","descending":false}))
            .await,
        (2, "ac".into())
    );
    assert_eq!(
        titles(json!({"filters":[{"field":"score","op":"eq","value":1},{"field":"score","op":"eq","value":3,"or":true},{"field":"title","op":"eq","value":"c"}]}))
            .await,
        (1, "c".into())
    );
    assert_eq!(
        titles(json!({"filters":[{"field":"title","op":"eq","value":"x"},{"field":"score","op":"empty","or":true},{"field":"double","op":"gt","value":5,"or":true}],"sort_field":"title","descending":false}))
            .await,
        (3, "cxy".into())
    );
    assert_eq!(titles(json!({"search":"Y"})).await, (1, "y".into()));

    // Renaming a field rebuilds the table with the values under the new name.
    let response = app
        .call(
            "PATCH",
            &format!("/api/v1/collections/{}", c.id),
            Some(&cookie),
            None,
            Some(json!({"blueprint":{"fields":[{"name":"title","type":"string","required":true},{"name":"points","type":"number","renamed_from":"score"}]}})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        titles(json!({"sort_field":"points","descending":true,"limit":3})).await,
        (5, "cba".into())
    );
    assert_eq!(
        app.call(
            "POST",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"sort_field":"score"}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    // Deleted records leave the table; a deleted collection drops it.
    let (_, page) = json_of::<vulse_model::RecordPage>(
        app.call(
            "POST",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"filters":[{"field":"title","op":"eq","value":"a"}]})),
        )
        .await,
    )
    .await;
    let response = app
        .call(
            "DELETE",
            &format!(
                "/api/v1/collections/{}/documents/{}",
                c.id, page.items[0].id
            ),
            Some(&cookie),
            None,
            None,
        )
        .await;
    assert!(response.status().is_success());
    assert_eq!(titles(json!({})).await.0, 4);
    let conn = app.db.connect().await.unwrap();
    let tables = async || {
        conn.query(
            "SELECT name FROM sqlite_schema WHERE type='table' AND name LIKE 'rows_%'",
            &[],
        )
        .await
        .unwrap()
        .len()
    };
    assert_eq!(tables().await, 1);
    // Rebuilding from the record bodies restores a lost table.
    conn.execute(&format!("DROP TABLE {}", crate::rows::table(&c.id)), &[])
        .await
        .unwrap();
    crate::records::rebuild_all_tables(&conn).await.unwrap();
    assert_eq!(
        titles(json!({"sort_field":"points","descending":false,"limit":1})).await,
        (4, "b".into())
    );
    let response = app
        .call(
            "DELETE",
            &format!("/api/v1/collections/{}", c.id),
            Some(&cookie),
            None,
            None,
        )
        .await;
    assert!(response.status().is_success());
    assert_eq!(tables().await, 0);
}

#[tokio::test]
async fn saved_views_query_filter_sort_page_and_recalculate() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "views@example.com", "views").await;
    let collection=create_collection(&app,&cookie,"products","private",json!({"fields":[{"name":"name","type":"string"},{"name":"price","type":"number"},{"name":"active","type":"boolean"},{"name":"total","type":"calculated","formula":"=[price]*2"}]})).await;
    for (name, price, active) in [
        ("Pen", 2, true),
        ("Notebook", 12, true),
        ("100%_ cotton", 5, false),
        ("Folder", 7, true),
    ] {
        create_document(
            &app,
            &cookie,
            &collection.id,
            json!({"name":name,"price":price,"active":active}),
        )
        .await;
    }
    let endpoint = format!("/api/v1/collections/{}/query", collection.id);
    let query = json!({"filters":[{"field":"active","op":"eq","value":true}],"sort_field":"price","descending":false,"limit":2});
    let (status, page) = json_of::<vulse_model::RecordPage>(
        app.call("POST", &endpoint, Some(&cookie), None, Some(query.clone()))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page.total, 3);
    assert_eq!(page.items[0].body["name"], "Pen");
    assert_eq!(page.items[1].body["name"], "Folder");
    let mut second = query.clone();
    second["offset"] = json!(2);
    let (_, page) = json_of::<vulse_model::RecordPage>(
        app.call("POST", &endpoint, Some(&cookie), None, Some(second))
            .await,
    )
    .await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].body["name"], "Notebook");
    let (_, page) = json_of::<vulse_model::RecordPage>(
        app.call(
            "POST",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"search":"100%_"})),
        )
        .await,
    )
    .await;
    assert_eq!(page.total, 1);
    let (_, page) = json_of::<vulse_model::RecordPage>(
        app.call(
            "POST",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"filters":[{"field":"total","op":"gt","value":20}]})),
        )
        .await,
    )
    .await;
    assert_eq!(page.total, 1);
    assert_eq!(
        app.call(
            "POST",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"sort_field":"name; DROP TABLE documents"}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let views = format!("/api/v1/collections/{}/views", collection.id);
    let config = json!({"name":"Active products","query":query,"columns":["name","price"]});
    let (status, saved) = json_of::<vulse_model::SavedView>(
        app.call("POST", &views, Some(&cookie), None, Some(config.clone()))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        app.call("POST", &views, Some(&cookie), None, Some(config))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let (_, seen) = json_of::<Vec<vulse_model::SavedView>>(
        app.call("GET", &views, Some(&cookie), None, None).await,
    )
    .await;
    assert_eq!(seen, vec![saved.clone()]);
    // Viewers can query and read saved views but not change them.
    let (viewer, _) =
        register_as(&app, "other-views@example.com", "otherviews", Role::Viewer).await;
    assert_eq!(
        app.call("POST", &endpoint, Some(&viewer), None, Some(json!({})))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call("GET", &views, Some(&viewer), None, None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call(
            "POST",
            &views,
            Some(&viewer),
            None,
            Some(json!({"name":"mine"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut blueprint = collection.blueprint.clone();
    blueprint.fields[3].formula = Some("=[price]*10".into());
    assert_eq!(
        app.call(
            "PATCH",
            &format!("/api/v1/collections/{}", collection.id),
            Some(&cookie),
            None,
            Some(json!({"blueprint":blueprint}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let (_, page) = json_of::<vulse_model::RecordPage>(
        app.call(
            "POST",
            &endpoint,
            Some(&cookie),
            None,
            Some(
                json!({"filters":[{"field":"total","op":"gt","value":20}],"group_field":"active"}),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(page.total, 3);
    assert_eq!(
        app.call(
            "DELETE",
            &format!("{views}/{}", saved.id),
            Some(&cookie),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn structured_portable_text_validates_nested_assets_and_public_round_trips() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "blocks@example.com", "blocks").await;
    let collection = create_collection(
        &app,
        &cookie,
        "articles",
        "public",
        json!({"fields":[{"name":"content", "type":"portable_text"}]}),
    )
    .await;
    let uploaded = multipart_call(
        &app,
        &cookie,
        &collection.id,
        "image/png",
        b"\x89PNG\r\nhello-image",
        "cover.png",
    )
    .await;
    let (status, asset) = json_of::<AssetView>(uploaded).await;
    assert_eq!(status, StatusCode::CREATED);
    let content = json!([
        {"_key":"accordion", "_type":"accordion", "title":"FAQ", "collapsed":true, "content":[
            {"_key":"repeat", "_type":"repeatable", "title":"Answers", "items":[
                {"_key":"entry", "label":"First", "content":[{"_key":"image", "_type":"image", "asset":{"_ref":asset.id}}]}
            ]}
        ]},
        {"_key":"embed", "_type":"iframe", "title":"Video", "url":"https://example.com/embed", "height":400}
    ]);
    let document = create_document(&app, &cookie, &collection.id, json!({"content":content})).await;
    let (_, public) = json_of::<DocumentView>(
        app.call(
            "GET",
            &format!("/api/v1/public/articles/{}", document.id),
            None,
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(public.body["content"], content);
    let asset_endpoint = format!("/api/v1/collections/{}/assets/{}", collection.id, asset.id);
    assert_eq!(
        app.call("DELETE", &asset_endpoint, Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "GET",
            &format!("{asset_endpoint}/content"),
            Some(&cookie),
            None,
            None
        )
        .await
        .status(),
        StatusCode::OK
    );

    let endpoint = format!(
        "/api/v1/collections/{}/documents/{}",
        collection.id, document.id
    );
    let mut invalid = content.clone();
    invalid[0]["content"][0]["items"][0]["content"][0]["asset"]["_ref"] =
        json!("550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(
        app.call(
            "PUT",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"body":{"content":invalid}}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    for url in [
        "javascript:alert(1)",
        "https://user:password@example.com/embed",
    ] {
        let mut invalid = content.clone();
        invalid[1]["url"] = json!(url);
        assert_eq!(
            app.call(
                "PUT",
                &endpoint,
                Some(&cookie),
                None,
                Some(json!({"body":{"content":invalid}}))
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let (_, saved) =
        json_of::<DocumentView>(app.call("GET", &endpoint, Some(&cookie), None, None).await).await;
    assert_eq!(saved.body["content"], content);
}

#[tokio::test]
async fn uploads_stay_inside_the_asset_directory() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let notes = create_collection(
        &app,
        &cookie,
        "notes",
        "public",
        json!({"fields": [{"name": "cover", "type": "asset", "required": true}]}),
    )
    .await;
    let svg = multipart_call(
        &app,
        &cookie,
        &notes.id,
        "image/svg+xml",
        b"<svg></svg>",
        "x.svg",
    )
    .await;
    assert_eq!(svg.status(), StatusCode::BAD_REQUEST);
    assert!(file_count(&app.asset_dir) == 0);

    let png = b"\x89PNG\r\nhello-image";
    let uploaded = multipart_call(&app, &cookie, &notes.id, "image/png", png, "cover.png").await;
    let (status, asset) = json_of::<AssetView>(uploaded).await;
    assert_eq!(status, StatusCode::CREATED);
    let stored = only_file(&app.asset_dir);
    let root = app.asset_dir.canonicalize().unwrap();
    assert!(stored.canonicalize().unwrap().starts_with(&root));
    assert_eq!(std::fs::read(&stored).unwrap(), png);

    let _doc = create_document(&app, &cookie, &notes.id, json!({"cover": asset.id})).await;
    let response = app
        .call(
            "GET",
            &format!("/api/v1/public/notes/assets/{}", asset.id),
            None,
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .unwrap(),
        "*"
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], png);

    let secrets = create_collection(
        &app,
        &cookie,
        "secrets",
        "private",
        json!({"fields": [{"name": "cover", "type": "asset"}]}),
    )
    .await;
    let private_asset = multipart_call(
        &app,
        &cookie,
        &secrets.id,
        "image/png",
        b"\x89PNG\r\nprivate",
        "secret.png",
    )
    .await;
    let (_, private_asset) = json_of::<AssetView>(private_asset).await;
    assert_eq!(
        app.call(
            "GET",
            &format!("/api/v1/public/secrets/assets/{}", private_asset.id),
            None,
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn json_bodies_are_limited() {
    let app = TestApp::new().await;
    let huge = "a".repeat(1024 * 1024 + 64);
    let response = app
        .call(
            "POST",
            "/api/v1/auth/login",
            None,
            None,
            Some(json!({"email": huge, "password": "password123"})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

async fn create_collection(
    app: &TestApp,
    cookie: &str,
    name: &str,
    visibility: &str,
    blueprint: Value,
) -> CollectionView {
    let response = app
        .call(
            "POST",
            "/api/v1/collections",
            Some(cookie),
            None,
            Some(json!({"name": name, "visibility": visibility, "blueprint": blueprint})),
        )
        .await;
    let (status, collection) = json_of::<CollectionView>(response).await;
    assert_eq!(status, StatusCode::CREATED, "{collection:?}");
    collection
}

async fn create_document(
    app: &TestApp,
    cookie: &str,
    collection_id: &str,
    body: Value,
) -> DocumentView {
    let response = app
        .call(
            "POST",
            &format!("/api/v1/collections/{collection_id}/documents"),
            Some(cookie),
            None,
            Some(json!({"body": body})),
        )
        .await;
    let (status, document) = json_of::<DocumentView>(response).await;
    assert_eq!(status, StatusCode::CREATED, "{document:?}");
    document
}

async fn multipart_call(
    app: &TestApp,
    cookie: &str,
    collection_id: &str,
    content_type: &str,
    bytes: &[u8],
    filename: &str,
) -> axum::response::Response {
    let boundary = "----vulseboundary";
    let mut body = Vec::new();
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .into_bytes(),
    );
    body.extend(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/collections/{collection_id}/assets"))
        .header("cookie", format!("vulse_session={cookie}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    app.app.clone().oneshot(request).await.unwrap()
}

fn file_count(root: &Path) -> usize {
    walk(root).len()
}

fn only_file(root: &Path) -> std::path::PathBuf {
    let files = walk(root);
    assert_eq!(files.len(), 1);
    files.into_iter().next().unwrap()
}

fn walk(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if !root.exists() {
        return files;
    }
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[allow(dead_code)]
fn _page<T>(page: Page<T>) -> Vec<T> {
    page.items
}

#[tokio::test]
async fn calculated_fields_refresh_on_writes_schema_changes_and_public_reads() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "calc@example.com", "calc").await;
    let collection = create_collection(
        &app,
        &cookie,
        "sales",
        "public",
        json!({"fields":[
            {"name":"price","type":"number","required":true}, {"name":"quantity","type":"number"},
            {"name":"total","type":"calculated","formula":"=ROUND([price]*[quantity],2)"},
            {"name":"safe","type":"calculated","formula":"=IFERROR(1/[quantity],0)"}
        ]}),
    )
    .await;
    let document = create_document(
        &app,
        &cookie,
        &collection.id,
        json!({"price":12,"quantity":3,"total":999}),
    )
    .await;
    assert_eq!(document.body["total"].as_f64(), Some(36.0));
    let url = format!(
        "/api/v1/collections/{}/documents/{}",
        collection.id, document.id
    );
    let (status, document) = json_of::<DocumentView>(
        app.call(
            "PUT",
            &url,
            Some(&cookie),
            None,
            Some(json!({"body":{"price":20,"quantity":0}})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(document.body["total"].as_f64(), Some(0.0));
    assert_eq!(document.body["safe"].as_f64(), Some(0.0));
    let response=app.call("PATCH",&format!("/api/v1/collections/{}",collection.id),Some(&cookie),None,Some(json!({"blueprint":{"fields":[
        {"name":"price","type":"number"},{"name":"quantity","type":"number"},
        {"name":"total","type":"calculated","formula":"=[price]+10"},{"name":"safe","type":"calculated","formula":"=1/[quantity]"}
    ]}}))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let (status, public) = json_of::<DocumentView>(
        app.call(
            "GET",
            &format!("/api/v1/public/sales/{}", document.id),
            None,
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public.body["total"].as_f64(), Some(30.0));
    assert!(public.body["safe"].is_null());
    assert!(public.calculation_errors["safe"].contains("DIV"));
    let (_, page) = json_of::<Page<DocumentView>>(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}/documents", collection.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(page.items[0].body["total"].as_f64(), Some(30.0));
    let bad=app.call("PATCH",&format!("/api/v1/collections/{}",collection.id),Some(&cookie),None,Some(json!({"blueprint":{"fields":[{"name":"a","type":"calculated","formula":"=[b]"},{"name":"b","type":"calculated","formula":"=[a]"}]}}))).await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
}

async fn import_csv(
    app: &TestApp,
    cookie: Option<&str>,
    id: &str,
    csv: &str,
    commit: bool,
    mapping: Option<Value>,
) -> axum::response::Response {
    import_csv_with(app, cookie, id, csv, commit, mapping, &[]).await
}

async fn import_csv_with(
    app: &TestApp,
    cookie: Option<&str>,
    id: &str,
    csv: &str,
    commit: bool,
    mapping: Option<Value>,
    options: &[(&str, &str)],
) -> axum::response::Response {
    let mut body = format!(
        "--vulse-import\r\nContent-Disposition: form-data; name=\"file\"; filename=\"data.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--vulse-import\r\nContent-Disposition: form-data; name=\"commit\"\r\n\r\n{commit}\r\n"
    );
    if let Some(m) = mapping {
        body.push_str(&format!(
            "--vulse-import\r\nContent-Disposition: form-data; name=\"mapping\"\r\n\r\n{m}\r\n"
        ));
    }
    for (name, value) in options {
        body.push_str(&format!(
            "--vulse-import\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str("--vulse-import--\r\n");
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/collections/{id}/import"))
        .header("content-type", "multipart/form-data; boundary=vulse-import");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("vulse_session={cookie}"));
    }
    app.app
        .clone()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}
#[tokio::test]
async fn fields_suggested_from_a_file_can_be_saved_and_then_imported() {
    use vulse_model::{FieldSuggestions, ImportResult};
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "fields@example.com", "fields").await;
    let c = create_collection(
        &app,
        &cookie,
        "pensjon",
        "private",
        json!({"fields":[{"name":"ar","type":"number"}]}),
    )
    .await;
    let csv = "År;Navn;Årlig premie;Startdato\n2024;Kari;12,5;2024-01-01\n2025;Ola;7;2025-02-01\n";
    let suggest = |cookie: Option<String>| {
        let body = format!(
            "--s\r\nContent-Disposition: form-data; name=\"file\"; filename=\"data.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--s--\r\n"
        );
        let mut builder = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/collections/{}/import/fields", c.id))
            .header("content-type", "multipart/form-data; boundary=s");
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", format!("vulse_session={cookie}"));
        }
        app.app
            .clone()
            .oneshot(builder.body(Body::from(body)).unwrap())
    };
    assert_eq!(
        suggest(None).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let (status, found) =
        json_of::<FieldSuggestions>(suggest(Some(cookie.clone())).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found.row_count, 2);
    let names: Vec<_> = found.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["ar", "navn", "arlig_premie", "startdato"]);
    // The client keeps the existing `ar` field and adds the rest, labeled
    // with their headers.
    let mut fields = vec![json!({"name":"ar","type":"number"})];
    for column in &found.columns[1..] {
        fields.push(json!({"name":column.name,"label":column.header,"type":column.kind}));
    }
    let saved = app
        .call(
            "PATCH",
            &format!("/api/v1/collections/{}", c.id),
            Some(&cookie),
            None,
            Some(json!({"blueprint":{"fields":fields}})),
        )
        .await;
    assert_eq!(saved.status(), StatusCode::OK);
    // Without a mapping, every column finds its field by name or label.
    let (_, done) =
        json_of::<ImportResult>(import_csv(&app, Some(&cookie), &c.id, csv, true, None).await)
            .await;
    assert!(done.errors.is_empty(), "{:?}", done.errors);
    assert_eq!(
        done.mapping,
        vec![
            Some("ar".to_string()),
            Some("navn".into()),
            Some("arlig_premie".into()),
            Some("startdato".into())
        ]
    );
    assert_eq!(done.imported, 2);
}
#[tokio::test]
async fn imports_files_without_a_header_row() {
    use vulse_model::ImportResult;
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "headerless@example.com", "headerless").await;
    let c = create_collection(
        &app,
        &cookie,
        "parts",
        "private",
        json!({"fields":[{"name":"title","type":"string","required":true},{"name":"price","type":"number","required":true}]}),
    )
    .await;
    let mapping = || Some(json!(["title", "price"]));
    let no_headers = [("has_headers", "false")];
    // The first line is data, so a bad value on line 2 is reported as row 2.
    let (status, preview) = json_of::<ImportResult>(
        import_csv_with(
            &app,
            Some(&cookie),
            &c.id,
            "Alpha,4\nBeta,NaN\n",
            false,
            mapping(),
            &no_headers,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview.headers, vec!["Column 1", "Column 2"]);
    assert_eq!(preview.row_count, 2);
    assert_eq!(preview.errors.len(), 1);
    assert_eq!(preview.errors[0].row, 2);
    assert_eq!(preview.errors[0].field.as_deref(), Some("price"));
    let (_, done) = json_of::<ImportResult>(
        import_csv_with(
            &app,
            Some(&cookie),
            &c.id,
            "Alpha,4\nBeta,5\n",
            true,
            mapping(),
            &no_headers,
        )
        .await,
    )
    .await;
    assert!(done.errors.is_empty());
    assert_eq!(done.imported, 2);
    let (_, page) = json_of::<Page<DocumentView>>(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}/documents", c.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    let mut titles: Vec<_> = page.items.iter().map(|d| d.body["title"].clone()).collect();
    titles.sort_by_key(|t| t.to_string());
    assert_eq!(titles, vec![json!("Alpha"), json!("Beta")]);
    assert_eq!(
        import_csv_with(
            &app,
            Some(&cookie),
            &c.id,
            "Gamma,1\n",
            false,
            mapping(),
            &[("has_headers", "no")]
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn imports_preview_validate_authorize_and_commit_atomically() {
    use vulse_model::ImportResult;
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "import@example.com", "import").await;
    let c=create_collection(&app,&cookie,"inventory","private",json!({"fields":[{"name":"title","type":"string","required":true},{"name":"price","type":"number","required":true},{"name":"tax","type":"calculated","formula":"=[price]*0.25"}]})).await;
    let csv = "title,price\n\"A, B\",12\nSecond,20\n";
    assert_eq!(
        import_csv(&app, None, &c.id, csv, true, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let (_, preview) =
        json_of::<ImportResult>(import_csv(&app, Some(&cookie), &c.id, csv, false, None).await)
            .await;
    assert!(preview.errors.is_empty());
    assert_eq!(preview.imported, 0);
    assert_eq!(preview.preview[0]["tax"].as_f64(), Some(3.0));
    let (_, page) = json_of::<Page<DocumentView>>(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}/documents", c.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert!(page.items.is_empty());
    let (_, bad) = json_of::<ImportResult>(
        import_csv(
            &app,
            Some(&cookie),
            &c.id,
            "title,price\nValid,4\nInvalid,NaN\nMissing,\n",
            true,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(bad.imported, 0);
    assert_eq!(bad.errors.len(), 2);
    assert_eq!(bad.errors[0].row, 3);
    assert_eq!(bad.errors[0].field.as_deref(), Some("price"));
    let (_, page) = json_of::<Page<DocumentView>>(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}/documents", c.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert!(page.items.is_empty());
    let (_, done) =
        json_of::<ImportResult>(import_csv(&app, Some(&cookie), &c.id, csv, true, None).await)
            .await;
    assert_eq!(done.imported, 2);
    let mapped = "Product,Cost,Other\nMapped,8,ignored\n";
    let (_, done) = json_of::<ImportResult>(
        import_csv(
            &app,
            Some(&cookie),
            &c.id,
            mapped,
            true,
            Some(json!(["title", "price", null])),
        )
        .await,
    )
    .await;
    assert_eq!(done.imported, 1);
    assert_eq!(
        import_csv(
            &app,
            Some(&cookie),
            &c.id,
            mapped,
            true,
            Some(json!(["title", "title", null]))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        import_csv(&app, Some(&cookie), &c.id, "title,title\na,b\n", true, None)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let (viewer, _) = register_as(&app, "other@example.com", "other", Role::Viewer).await;
    assert_eq!(
        import_csv(&app, Some(&viewer), &c.id, csv, true, None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn history_restore_deleted_records_and_schema_conflicts() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "history@example.com", "history").await;
    let c=create_collection(&app,&cookie,"products","private",json!({"fields":[{"name":"name","type":"string"},{"name":"price","type":"number"},{"name":"total","type":"calculated","formula":"=[price]*2"}]})).await;
    let d = create_document(&app, &cookie, &c.id, json!({"name":"Notebook","price":2})).await;
    let endpoint = format!("/api/v1/collections/{}/documents/{}", c.id, d.id);
    let (status, history) = json_of::<Page<vulse_model::DocumentRevision>>(
        app.call(
            "GET",
            &format!("{endpoint}/revisions"),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history.items.len(), 1);
    assert_eq!(history.items[0].action, "create");
    assert!(history.items[0].body.get("total").is_none());
    let (status, updated) = json_of::<DocumentView>(
        app.call(
            "PUT",
            &endpoint,
            Some(&cookie),
            None,
            Some(json!({"body":{"name":"Changed","price":3},"expected_updated_at":d.updated_at})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let restore = format!("{endpoint}/revisions/{}/restore", history.items[0].id);
    assert_eq!(
        app.call(
            "POST",
            &restore,
            Some(&cookie),
            None,
            Some(json!({"expected_updated_at":d.updated_at}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let (status, restored) = json_of::<DocumentView>(
        app.call(
            "POST",
            &restore,
            Some(&cookie),
            None,
            Some(json!({"expected_updated_at":updated.updated_at})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored.body["name"], "Notebook");
    assert_eq!(restored.body["total"].as_f64(), Some(4.0));
    assert_eq!(
        app.call("DELETE", &endpoint, Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let (_, trash) = json_of::<Page<vulse_model::DocumentRevision>>(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}/deleted", c.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(trash.items.len(), 1);
    let (status, recreated) = json_of::<DocumentView>(
        app.call("POST", &restore, Some(&cookie), None, Some(json!({})))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recreated.id, d.id);
    assert_eq!(recreated.created_at, d.created_at);
    let collection_endpoint = format!("/api/v1/collections/{}", c.id);
    assert_eq!(
        app.call(
            "PATCH",
            &collection_endpoint,
            Some(&cookie),
            None,
            Some(json!({"name":"renamed","expected_updated_at":c.updated_at}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call(
            "PATCH",
            &collection_endpoint,
            Some(&cookie),
            None,
            Some(json!({"visibility":"public","expected_updated_at":c.updated_at}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let (viewer, _) = register_as(
        &app,
        "historyother@example.com",
        "historyother",
        Role::Viewer,
    )
    .await;
    assert_eq!(
        app.call(
            "GET",
            &format!("{endpoint}/revisions"),
            Some(&viewer),
            None,
            None
        )
        .await
        .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn relationship_rename_backlinks_and_deletion_guards() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "links@example.com", "links").await;
    let products = create_collection(
        &app,
        &cookie,
        "products",
        "private",
        json!({"title_field":"name","fields":[{"name":"name","type":"string"}]}),
    )
    .await;
    let target = create_document(&app, &cookie, &products.id, json!({"name":"Notebook"})).await;
    let orders=create_collection(&app,&cookie,"orders","private",json!({"title_field":"name","fields":[{"name":"name","type":"string"},{"name":"product","type":"reference","collection":"products","label":"Ordered product"}]})).await;
    let source = create_document(
        &app,
        &cookie,
        &orders.id,
        json!({"name":"Order #1","product":target.id}),
    )
    .await;
    let endpoint = format!(
        "/api/v1/collections/{}/documents/{}",
        products.id, target.id
    );
    let (_, links) = json_of::<Page<vulse_model::ReferenceView>>(
        app.call(
            "GET",
            &format!("{endpoint}/references"),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(links.items.len(), 1);
    assert_eq!(links.items[0].title, "Order #1");
    assert_eq!(links.items[0].field, "Ordered product");
    let (_, key) = json_of::<CreatedApiKey>(
        app.call(
            "POST",
            "/api/v1/api-keys",
            Some(&cookie),
            None,
            Some(json!({"name":"products only","collection_id":products.id})),
        )
        .await,
    )
    .await;
    let (status, scoped) = json_of::<Page<vulse_model::ReferenceView>>(
        app.call(
            "GET",
            &format!("{endpoint}/references"),
            None,
            Some(&key.secret),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(scoped.items.is_empty());
    let (_, duplicate) = json_of::<vulse_model::MutationResult>(
        app.call(
            "POST",
            &format!("/api/v1/collections/{}/bulk", products.id),
            Some(&cookie),
            None,
            Some(json!({"action":"duplicate","ids":[target.id]})),
        )
        .await,
    )
    .await;
    let copy = &duplicate.documents[0];
    let order_copy = create_document(
        &app,
        &cookie,
        &orders.id,
        json!({"name":"Order copy","product":copy.id}),
    )
    .await;
    let undo = format!(
        "/api/v1/collections/{}/actions/{}/undo",
        products.id, duplicate.undo_id
    );
    assert_eq!(
        app.call("POST", &undo, Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "PATCH",
            &format!(
                "/api/v1/collections/{}/documents/{}",
                orders.id, order_copy.id
            ),
            Some(&cookie),
            None,
            Some(json!({"patch":{"product":null}}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call("POST", &undo, Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::OK
    );

    assert_eq!(
        app.call("DELETE", &endpoint, Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "DELETE",
            &format!("/api/v1/collections/{}", products.id),
            Some(&cookie),
            None,
            None
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "PATCH",
            &format!("/api/v1/collections/{}", products.id),
            Some(&cookie),
            None,
            Some(json!({"name":"catalog"}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let (_, renamed) = json_of::<CollectionView>(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}", orders.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        renamed.blueprint.fields[1].collection.as_deref(),
        Some("catalog")
    );
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/collections/{}/documents/{}", orders.id, source.id),
            Some(&cookie),
            None,
            Some(json!({"body":{"name":"Renamed order","product":target.id}}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call(
            "PATCH",
            &format!("/api/v1/collections/{}/documents/{}", orders.id, source.id),
            Some(&cookie),
            None,
            Some(json!({"patch":{"product":null}}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call("DELETE", &endpoint, Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let selfcoll=create_collection(&app,&cookie,"nodes","private",json!({"fields":[{"name":"name","type":"string"},{"name":"parent","type":"reference","collection":"nodes"}]})).await;
    let a = create_document(&app, &cookie, &selfcoll.id, json!({"name":"A"})).await;
    let b = create_document(
        &app,
        &cookie,
        &selfcoll.id,
        json!({"name":"B","parent":a.id}),
    )
    .await;
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/collections/{}/bulk", selfcoll.id),
            Some(&cookie),
            None,
            Some(json!({"action":"delete","ids":[a.id]}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/collections/{}/bulk", selfcoll.id),
            Some(&cookie),
            None,
            Some(json!({"action":"delete","ids":[a.id,b.id]}))
        )
        .await
        .status(),
        StatusCode::OK
    );
}

async fn upsert_csv(
    app: &TestApp,
    cookie: &str,
    id: &str,
    csv: &str,
    commit: bool,
    field: &str,
) -> axum::response::Response {
    let body = format!(
        "--upsert\r\nContent-Disposition: form-data; name=\"file\"; filename=\"data.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--upsert\r\nContent-Disposition: form-data; name=\"commit\"\r\n\r\n{commit}\r\n--upsert\r\nContent-Disposition: form-data; name=\"mode\"\r\n\r\nupsert\r\n--upsert\r\nContent-Disposition: form-data; name=\"match_field\"\r\n\r\n{field}\r\n--upsert--\r\n"
    );
    app.app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/collections/{id}/import"))
                .header("cookie", format!("vulse_session={cookie}"))
                .header("content-type", "multipart/form-data; boundary=upsert")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn upsert_imports_preview_counts_validate_keys_preserve_fields_and_export_views() {
    use vulse_model::ImportResult;
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "upsert@example.com", "upsert").await;
    let c=create_collection(&app,&cookie,"products","private",json!({"fields":[{"name":"sku","type":"string","required":true},{"name":"name","type":"string"},{"name":"price","type":"number","required":true},{"name":"total","type":"calculated","formula":"=[price]*2"}]})).await;
    let first = create_document(
        &app,
        &cookie,
        &c.id,
        json!({"sku":"A","name":"A, \"quoted\"\nNotebook","price":2}),
    )
    .await;
    let second = create_document(
        &app,
        &cookie,
        &c.id,
        json!({"sku":"B","name":"=1+2","price":3}),
    )
    .await;
    let csv = "sku,price\nA,5\nB,3\nC,7\n";
    let (status, preview) =
        json_of::<ImportResult>(upsert_csv(&app, &cookie, &c.id, csv, false, "sku").await).await;
    assert_eq!(status, StatusCode::OK);
    assert!(preview.errors.is_empty());
    assert_eq!(
        (
            preview.created,
            preview.updated,
            preview.unchanged,
            preview.imported
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(
        preview.preview_actions,
        vec!["update", "unchanged", "create"]
    );
    assert_eq!(preview.preview[0]["name"], first.body["name"]);
    assert_eq!(preview.preview[0]["total"].as_f64(), Some(10.0));
    let (_, bad) = json_of::<ImportResult>(
        upsert_csv(&app, &cookie, &c.id, "sku,price\nA,8\nA,9\n", true, "sku").await,
    )
    .await;
    assert!(!bad.errors.is_empty());
    assert_eq!(bad.imported, 0);
    let endpoint = format!("/api/v1/collections/{}/documents/{}", c.id, first.id);
    let (_, unchanged) =
        json_of::<DocumentView>(app.call("GET", &endpoint, Some(&cookie), None, None).await).await;
    assert_eq!(unchanged.updated_at, first.updated_at);
    let (_, done) =
        json_of::<ImportResult>(upsert_csv(&app, &cookie, &c.id, csv, true, "sku").await).await;
    assert!(done.errors.is_empty());
    assert_eq!(done.imported, 2);
    let (_, updated) =
        json_of::<DocumentView>(app.call("GET", &endpoint, Some(&cookie), None, None).await).await;
    assert_eq!(updated.id, first.id);
    assert_eq!(updated.body["name"], first.body["name"]);
    assert_eq!(updated.body["total"].as_f64(), Some(10.0));
    let (_, repeat) =
        json_of::<ImportResult>(upsert_csv(&app, &cookie, &c.id, csv, true, "sku").await).await;
    assert_eq!(
        (
            repeat.created,
            repeat.updated,
            repeat.unchanged,
            repeat.imported
        ),
        (0, 0, 3, 0)
    );
    let (_, blank) = json_of::<ImportResult>(
        upsert_csv(&app, &cookie, &c.id, "sku,price,name\nA,,\n", true, "sku").await,
    )
    .await;
    assert!(blank.errors.is_empty());
    assert_eq!(blank.unchanged, 1);
    let (_, missing) = json_of::<ImportResult>(
        upsert_csv(&app, &cookie, &c.id, "sku,price\n,9\n", true, "sku").await,
    )
    .await;
    assert!(!missing.errors.is_empty());
    assert_eq!(missing.imported, 0);
    let export = format!("/api/v1/collections/{}/export", c.id);
    let response=app.call("POST",&export,Some(&cookie),None,Some(json!({"format":"csv","columns":["sku","name","total"],"query":{"filters":[{"field":"price","op":"gt","value":4}],"sort_field":"sku","descending":false,"offset":1,"limit":1}}))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("products.csv")
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let mut csv = csv::Reader::from_reader(bytes.as_ref());
    assert_eq!(
        csv.headers().unwrap(),
        &csv::StringRecord::from(vec!["sku", "name", "total"])
    );
    let rows = csv.records().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(&rows[0][0], "A");
    assert_eq!(&rows[0][1], "A, \"quoted\"\nNotebook");
    assert_eq!(&rows[1][0], "C");
    let response = app
        .call(
            "POST",
            &export,
            Some(&cookie),
            None,
            Some(json!({"format":"csv","columns":["name"],"ids":[second.id]})),
        )
        .await;
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&bytes).contains("'=1+2"));
    let(status,selected)=json_of::<Vec<DocumentView>>(app.call("POST",&export,Some(&cookie),None,Some(json!({"format":"json","columns":["name"],"ids":[second.id],"query":{"search":"does not match"}}))).await).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].body, json!({"name":"=1+2"}));
    assert_eq!(selected[0].id, second.id);
    assert_eq!(
        app.call(
            "POST",
            &export,
            Some(&cookie),
            None,
            Some(json!({"format":"csv","columns":["sku; DROP TABLE documents"]}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let (viewer, _) =
        register_as(&app, "upsertother@example.com", "upsertother", Role::Viewer).await;
    assert_eq!(
        app.call(
            "POST",
            &export,
            Some(&viewer),
            None,
            Some(json!({"format":"csv","columns":["sku"]}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    // Numeric matching is canonical, while integers above 2^53 remain distinct.
    let nums = create_collection(
        &app,
        &cookie,
        "numbers",
        "private",
        json!({"fields":[{"name":"key","type":"number"},{"name":"name","type":"string"}]}),
    )
    .await;
    create_document(&app, &cookie, &nums.id, json!({"key":1,"name":"One"})).await;
    create_document(
        &app,
        &cookie,
        &nums.id,
        json!({"key":9007199254740992u64,"name":"Large"}),
    )
    .await;
    let (_, numeric) = json_of::<ImportResult>(
        upsert_csv(
            &app,
            &cookie,
            &nums.id,
            "key,name\n1.0,One\n9007199254740993,Distinct\n",
            true,
            "key",
        )
        .await,
    )
    .await;
    assert!(numeric.errors.is_empty());
    assert_eq!((numeric.created, numeric.unchanged), (1, 1));
    create_document(&app, &cookie, &c.id, json!({"sku":"A","price":1})).await;
    let (_, duplicate) = json_of::<ImportResult>(
        upsert_csv(&app, &cookie, &c.id, "sku,price\nA,9\n", true, "sku").await,
    )
    .await;
    assert!(!duplicate.errors.is_empty());
    assert_eq!(duplicate.imported, 0);
}

#[tokio::test]
async fn authentication_work_is_bounded_and_releases_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let state = crate::test_state(dir.path().to_path_buf()).await.unwrap();
    let semaphore = state.password_work.clone();
    let app = TestApp::with_state(state, dir);
    register(&app, "busy@example.com", "busy").await;
    let permit = semaphore.clone().acquire_many_owned(4).await.unwrap();
    let response = login(&app, "busy@example.com", "password123").await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(permit);
    let response = login(&app, "busy@example.com", "password123").await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = session_of(&response);
    assert_eq!(semaphore.available_permits(), 4);
    assert_eq!(
        app.call("GET", "/api/v1/me", Some(&cookie), None, None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        login(&app, "busy@example.com", "wrongpassword")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&app, "nobody@example.com", "wrongpassword")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(semaphore.available_permits(), 4);
}

#[tokio::test]
async fn setup_creates_one_admin_with_the_token() {
    let app = TestApp::new().await;
    let (_, status) = json_of::<vulse_model::SetupStatus>(
        app.call("GET", "/api/v1/auth/setup", None, None, None)
            .await,
    )
    .await;
    assert!(status.needs_setup);
    let wrong = app
        .call(
            "POST",
            "/api/v1/auth/setup",
            None,
            None,
            Some(json!({"token":"wrong","email":"eve@example.com","handle":"eve","password":"password123"})),
        )
        .await;
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        app.call("POST", "/api/v1/auth/register", None, None, Some(json!({})))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let (admin, user) = register(&app, "ada@example.com", "ada").await;
    assert_eq!(user.role, Role::Admin);
    let again = app
        .call(
            "POST",
            "/api/v1/auth/setup",
            None,
            None,
            Some(json!({"token":crate::TEST_SETUP_TOKEN,"email":"eve@example.com","handle":"eve","password":"password123"})),
        )
        .await;
    assert_eq!(again.status(), StatusCode::CONFLICT);
    let (_, status) = json_of::<vulse_model::SetupStatus>(
        app.call("GET", "/api/v1/auth/setup", None, None, None)
            .await,
    )
    .await;
    assert!(!status.needs_setup);

    // Sessions are stored as hashes, never as the cookie value.
    let conn = app.db.connect().await.unwrap();
    let rows = conn
        .query("SELECT token_hash FROM auth_sessions", &[])
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let stored = crate::db::text_at(&rows[0], 0).unwrap();
    assert_ne!(stored, admin);
    assert_eq!(stored, crate::auth::hash_secret(&admin));
}

#[tokio::test]
async fn invites_are_single_use_and_admin_only() {
    let app = TestApp::new().await;
    let (admin, _) = register(&app, "ada@example.com", "ada").await;
    let created = invite(&app, &admin, "Grace@Example.com", Role::Editor).await;
    assert_eq!(created.invite.email, "grace@example.com");
    let (status, invites) = json_of::<Vec<vulse_model::InviteView>>(
        app.call("GET", "/api/v1/invites", Some(&admin), None, None)
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(invites.len(), 1);
    assert!(!format!("{invites:?}").contains(&created.token));

    let (status, info) = json_of::<vulse_model::LinkInfo>(
        app.call(
            "POST",
            "/api/v1/auth/links/inspect",
            None,
            None,
            Some(json!({"token": created.token})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info.purpose, vulse_model::LinkPurpose::Invite);
    assert_eq!(info.email, "grace@example.com");

    assert_eq!(
        accept(&app, &created.token, None, "password123")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let response = accept(&app, &created.token, Some("grace"), "password123").await;
    assert_eq!(response.status(), StatusCode::OK);
    let editor = session_of(&response);
    assert_eq!(
        accept(&app, &created.token, Some("grace2"), "password123")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    // Only admin sessions manage people; API keys never do.
    assert_eq!(
        app.call("GET", "/api/v1/users", Some(&editor), None, None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let (_, key) = json_of::<CreatedApiKey>(
        app.call(
            "POST",
            "/api/v1/api-keys",
            Some(&admin),
            None,
            Some(json!({"name":"admin key"})),
        )
        .await,
    )
    .await;
    assert_eq!(key.key.role, Role::Admin);
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/invites",
            None,
            Some(&key.secret),
            Some(json!({"email":"x@example.com","role":"viewer"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    // A replaced or revoked invite stops working.
    let first = invite(&app, &admin, "linus@example.com", Role::Viewer).await;
    let second = invite(&app, &admin, "linus@example.com", Role::Viewer).await;
    assert_eq!(
        accept(&app, &first.token, Some("linus"), "password123")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.call(
            "DELETE",
            &format!("/api/v1/invites/{}", second.invite.id),
            Some(&admin),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        accept(&app, &second.token, Some("linus"), "password123")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn roles_limit_reads_writes_and_schema_changes() {
    let app = TestApp::new().await;
    let (admin, _) = register(&app, "ada@example.com", "ada").await;
    let (editor, _) = register_as(&app, "ed@example.com", "ed", Role::Editor).await;
    let (viewer, _) = register_as(&app, "vi@example.com", "vi", Role::Viewer).await;
    let posts = create_collection(
        &app,
        &admin,
        "posts",
        "private",
        json!({"fields":[{"name":"title","type":"string"}]}),
    )
    .await;
    let documents = format!("/api/v1/collections/{}/documents", posts.id);
    let schema = json!({"name":"drafts","visibility":"private","blueprint":{"fields":[]}});

    for cookie in [&admin, &editor, &viewer] {
        assert_eq!(
            app.call("GET", &documents, Some(cookie), None, None)
                .await
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        app.call(
            "POST",
            &documents,
            Some(&viewer),
            None,
            Some(json!({"body":{"title":"No"}}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let doc = create_document(&app, &editor, &posts.id, json!({"title":"Yes"})).await;
    assert_eq!(
        app.call(
            "DELETE",
            &format!("{documents}/{}", doc.id),
            Some(&viewer),
            None,
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for cookie in [&editor, &viewer] {
        assert_eq!(
            app.call(
                "POST",
                "/api/v1/collections",
                Some(cookie),
                None,
                Some(schema.clone())
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.call(
                "DELETE",
                &format!("/api/v1/collections/{}", posts.id),
                Some(cookie),
                None,
                None
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }

    // Collection names are unique across the workspace.
    let duplicate = app
        .call(
            "POST",
            "/api/v1/collections",
            Some(&admin),
            None,
            Some(json!({"name":"posts","visibility":"private","blueprint":{"fields":[]}})),
        )
        .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    // A key cannot exceed its creator, and follows the creator's current role.
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/api-keys",
            Some(&editor),
            None,
            Some(json!({"name":"too much","role":"admin"}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let (_, key) = json_of::<CreatedApiKey>(
        app.call(
            "POST",
            "/api/v1/api-keys",
            Some(&editor),
            None,
            Some(json!({"name":"site"})),
        )
        .await,
    )
    .await;
    assert_eq!(key.key.role, Role::Editor);
    let (_, read_key) = json_of::<CreatedApiKey>(
        app.call(
            "POST",
            "/api/v1/api-keys",
            Some(&editor),
            None,
            Some(json!({"name":"read","role":"viewer"})),
        )
        .await,
    )
    .await;
    assert_eq!(
        app.call(
            "POST",
            &documents,
            None,
            Some(&read_key.secret),
            Some(json!({"body":{"title":"No"}}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    create_document_with_key(&app, &key.secret, &posts.id).await;
    let editor_id = user_id(&app, &admin, "ed@example.com").await;
    let (status, _) = json_of::<ManagedUser>(
        app.call(
            "PATCH",
            &format!("/api/v1/users/{editor_id}"),
            Some(&admin),
            None,
            Some(json!({"role":"viewer"})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, me) = json_of::<UserView>(
        app.call("GET", "/api/v1/me", None, Some(&key.secret), None)
            .await,
    )
    .await;
    assert_eq!(me.role, Role::Viewer);
    assert_eq!(
        app.call(
            "POST",
            &documents,
            None,
            Some(&key.secret),
            Some(json!({"body":{"title":"No"}}))
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

async fn create_document_with_key(app: &TestApp, secret: &str, collection: &str) {
    assert_eq!(
        app.call(
            "POST",
            &format!("/api/v1/collections/{collection}/documents"),
            None,
            Some(secret),
            Some(json!({"body":{"title":"From key"}}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
}

async fn user_id(app: &TestApp, admin: &str, email: &str) -> String {
    let (_, users) = json_of::<Vec<ManagedUser>>(
        app.call("GET", "/api/v1/users", Some(admin), None, None)
            .await,
    )
    .await;
    users
        .into_iter()
        .find(|user| user.email == email)
        .unwrap()
        .id
}

#[tokio::test]
async fn disabling_ends_access_and_one_admin_remains() {
    let app = TestApp::new().await;
    let (admin, admin_user) = register(&app, "ada@example.com", "ada").await;
    let (editor, editor_user) = register_as(&app, "ed@example.com", "ed", Role::Editor).await;
    let (_, key) = json_of::<CreatedApiKey>(
        app.call(
            "POST",
            "/api/v1/api-keys",
            Some(&editor),
            None,
            Some(json!({"name":"site"})),
        )
        .await,
    )
    .await;
    let update = |id: String, body: Value| {
        let admin = admin.clone();
        let app = &app;
        async move {
            app.call(
                "PATCH",
                &format!("/api/v1/users/{id}"),
                Some(&admin),
                None,
                Some(body),
            )
            .await
            .status()
        }
    };
    assert_eq!(
        update(editor_user.id.clone(), json!({"disabled":true})).await,
        StatusCode::OK
    );
    for (cookie, bearer) in [
        (Some(editor.as_str()), None),
        (None, Some(key.secret.as_str())),
    ] {
        assert_eq!(
            app.call("GET", "/api/v1/me", cookie, bearer, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        login(&app, "ed@example.com", "password123").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        update(editor_user.id.clone(), json!({"disabled":false})).await,
        StatusCode::OK
    );
    assert_eq!(
        app.call("GET", "/api/v1/me", None, Some(&key.secret), None)
            .await
            .status(),
        StatusCode::OK
    );

    assert_eq!(
        update(admin_user.id.clone(), json!({"role":"editor"})).await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        update(admin_user.id.clone(), json!({"disabled":true})).await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        update(editor_user.id.clone(), json!({"role":"admin"})).await,
        StatusCode::OK
    );
    assert_eq!(
        update(admin_user.id.clone(), json!({"role":"viewer"})).await,
        StatusCode::OK
    );
    assert_eq!(
        app.call("GET", "/api/v1/users", Some(&admin), None, None)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn passwords_change_reset_and_end_other_sessions() {
    let app = TestApp::new().await;
    let (admin, _) = register(&app, "ada@example.com", "ada").await;
    let (first, user) = register_as(&app, "ed@example.com", "ed", Role::Editor).await;
    let second = session_of(&login(&app, "ed@example.com", "password123").await);

    let change = |cookie: String, current: &'static str| {
        let app = &app;
        async move {
            app.call(
                "POST",
                "/api/v1/auth/password",
                Some(&cookie),
                None,
                Some(json!({"current_password": current, "new_password": "new-password-1"})),
            )
            .await
            .status()
        }
    };
    assert_eq!(
        change(first.clone(), "wrong-password").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        change(first.clone(), "password123").await,
        StatusCode::NO_CONTENT
    );
    let me = |cookie: String| {
        let app = &app;
        async move {
            app.call("GET", "/api/v1/me", Some(&cookie), None, None)
                .await
                .status()
        }
    };
    assert_eq!(me(first.clone()).await, StatusCode::OK);
    assert_eq!(me(second.clone()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        login(&app, "ed@example.com", "password123").await.status(),
        StatusCode::UNAUTHORIZED
    );

    // An admin reset link sets a new password and signs out everywhere.
    let (status, reset) = json_of::<CreatedInvite>(
        app.call(
            "POST",
            &format!("/api/v1/users/{}/password-reset", user.id),
            Some(&admin),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(reset.invite.purpose, vulse_model::LinkPurpose::Reset);
    let response = accept(&app, &reset.token, None, "reset-password-1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let third = session_of(&response);
    assert_eq!(me(first.clone()).await, StatusCode::UNAUTHORIZED);
    assert_eq!(me(third.clone()).await, StatusCode::OK);
    assert_eq!(
        login(&app, "ed@example.com", "reset-password-1")
            .await
            .status(),
        StatusCode::OK
    );

    assert_eq!(
        app.call(
            "POST",
            "/api/v1/auth/logout-everywhere",
            Some(&third),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(me(third).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn failed_sign_ins_are_throttled_per_account() {
    let app = TestApp::new().await;
    register(&app, "ada@example.com", "ada").await;
    for _ in 0..crate::security::ACCOUNT_LIMIT {
        assert_eq!(
            login(&app, "ada@example.com", "wrong-password")
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let blocked = login(&app, "ada@example.com", "password123").await;
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = blocked
        .headers()
        .get("retry-after")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(retry > 0 && retry <= crate::security::WINDOW.as_secs());
    // Unknown accounts are counted the same way.
    for _ in 0..crate::security::ACCOUNT_LIMIT {
        login(&app, "ghost@example.com", "wrong-password").await;
    }
    assert_eq!(
        login(&app, "ghost@example.com", "wrong-password")
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn cross_site_cookie_writes_are_rejected() {
    let app = TestApp::new().await;
    let (admin, _) = register(&app, "ada@example.com", "ada").await;
    let body = json!({"name":"notes","visibility":"private","blueprint":{"fields":[]}});
    for headers in [
        vec![("sec-fetch-site", "cross-site")],
        vec![("sec-fetch-site", "same-site")],
        vec![("origin", "https://evil.example"), ("host", "cms.example")],
        vec![("origin", "null"), ("host", "cms.example")],
    ] {
        assert_eq!(
            app.call_with(
                "POST",
                "/api/v1/collections",
                Some(&admin),
                None,
                Some(body.clone()),
                &headers
            )
            .await
            .status(),
            StatusCode::FORBIDDEN,
            "{headers:?}"
        );
    }
    // Reads, same-origin writes and bearer requests are unaffected.
    assert_eq!(
        app.call_with(
            "GET",
            "/api/v1/me",
            Some(&admin),
            None,
            None,
            &[("sec-fetch-site", "cross-site")]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.call_with(
            "POST",
            "/api/v1/collections",
            Some(&admin),
            None,
            Some(body),
            &[("origin", "http://cms.example"), ("host", "cms.example")]
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    let (_, key) = json_of::<CreatedApiKey>(
        app.call(
            "POST",
            "/api/v1/api-keys",
            Some(&admin),
            None,
            Some(json!({"name":"site"})),
        )
        .await,
    )
    .await;
    assert_eq!(
        app.call_with(
            "POST",
            "/api/v1/collections",
            None,
            Some(&key.secret),
            Some(json!({"name":"other","visibility":"private","blueprint":{"fields":[]}})),
            &[("sec-fetch-site", "cross-site")]
        )
        .await
        .status(),
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn secure_cookies_use_the_host_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = crate::test_state(dir.path().to_path_buf()).await.unwrap();
    state.auth = std::sync::Arc::new(crate::state::AuthSettings {
        cookie_secure: true,
        setup_token_hash: state.auth.setup_token_hash.clone(),
        trust_proxy: false,
        public_origin: None,
        password_wait: state.auth.password_wait,
        base_url: state.auth.base_url.clone(),
    });
    let app = TestApp::with_state(state, dir);
    let response = app
        .call(
            "POST",
            "/api/v1/auth/setup",
            None,
            None,
            Some(json!({"token": crate::TEST_SETUP_TOKEN, "email": "ada@example.com", "handle": "ada", "password": "password123"})),
        )
        .await;
    let raw = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(raw.starts_with("__Host-vulse_session="));
    assert!(raw.contains("; Secure"));
    let token = raw
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("__Host-vulse_session=");
    let me = Request::builder()
        .uri("/api/v1/me")
        .header("cookie", format!("__Host-vulse_session={token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.app.clone().oneshot(me).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn admin_pages_cannot_be_framed() {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().join("dist");
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("index.html"), "<html></html>").unwrap();
    let mut state = crate::test_state(dir.path().to_path_buf()).await.unwrap();
    state.web_dist = dist;
    let app = TestApp::with_state(state, dir);
    let page = app.call("GET", "/setup", None, None, None).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers().get("x-frame-options").unwrap(), "DENY");
    let api = app
        .call("GET", "/api/v1/auth/setup", None, None, None)
        .await;
    assert!(api.headers().get("x-frame-options").is_none());
}

#[tokio::test]
async fn migrations_backfill_legacy_history_search_and_relationships() {
    use crate::db::{Db, SqlValue, text_at};
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open_local(&dir.path().join("legacy.db")).await.unwrap();
    let conn = db.connect().await.unwrap();
    conn.execute_batch(include_str!("../migrations/001_init.sql"))
        .await
        .unwrap();
    let source = "550e8400-e29b-41d4-a716-446655440001";
    let target = "550e8400-e29b-41d4-a716-446655440002";
    let blueprint = json!({"fields":[{"name":"name","type":"string"},{"name":"parent","type":"reference","collection":"nodes"}]});
    conn.execute("INSERT INTO users(id,handle,email,password_hash,created_at) VALUES('owner','legacy','legacy@example.com','unused','2026-01-01T00:00:00Z')",&[]).await.unwrap();
    conn.execute("INSERT INTO collections(id,owner_id,name,visibility,blueprint,created_at,updated_at) VALUES('collection','owner','nodes','private',?1,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",&[SqlValue::Text(blueprint.to_string())]).await.unwrap();
    for (id, body) in [
        (target, json!({"name":"Parent"})),
        (source, json!({"name":"Child","parent":target})),
    ] {
        conn.execute("INSERT INTO documents(id,collection_id,body,created_at,updated_at) VALUES(?1,'collection',?2,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",&[SqlValue::Text(id.into()),SqlValue::Text(body.to_string())]).await.unwrap();
    }
    // A second account with the same collection name, plus a collection that
    // references it, from when names were unique per account.
    conn.execute("INSERT INTO users(id,handle,email,password_hash,created_at) VALUES('second','second','second@example.com','unused','2026-01-02T00:00:00Z')",&[]).await.unwrap();
    conn.execute("INSERT INTO collections(id,owner_id,name,visibility,blueprint,created_at,updated_at) VALUES('second-nodes','second','nodes','private',?1,'2026-01-02T00:00:00Z','2026-01-02T00:00:00Z')",&[SqlValue::Text(blueprint.to_string())]).await.unwrap();
    let linking = json!({"fields":[{"name":"node","type":"reference","collection":"nodes"}]});
    conn.execute("INSERT INTO collections(id,owner_id,name,visibility,blueprint,created_at,updated_at) VALUES('second-links','second','links','private',?1,'2026-01-02T00:00:00Z','2026-01-02T00:00:00Z')",&[SqlValue::Text(linking.to_string())]).await.unwrap();
    conn.execute("INSERT INTO sessions(token,user_id,expires_at) VALUES('plain','owner','2099-01-01T00:00:00Z')",&[]).await.unwrap();
    conn.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL);INSERT INTO schema_migrations VALUES(1,'2026-01-01T00:00:00Z');").await.unwrap();
    crate::migrate::migrate(&conn).await.unwrap();
    crate::migrate::migrate(&conn).await.unwrap();
    let links = conn
        .query("SELECT source_id,target_id FROM document_links", &[])
        .await
        .unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(text_at(&links[0], 0).unwrap(), source);
    assert_eq!(text_at(&links[0], 1).unwrap(), target);
    let history = conn
        .query("SELECT action FROM record_revisions", &[])
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().all(|r| text_at(r, 0).unwrap() == "baseline"));
    let c = crate::store::find_collection(&conn, "collection")
        .await
        .unwrap()
        .unwrap();
    let result = crate::records::run_query(
        &conn,
        &c,
        &vulse_model::RecordQuery {
            search: "Parent".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.items[0].id, target);

    // Existing accounts keep full control, and names are now workspace-wide.
    let roles = conn
        .query("SELECT DISTINCT role FROM users", &[])
        .await
        .unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(text_at(&roles[0], 0).unwrap(), "admin");
    let renamed = crate::store::find_collection(&conn, "second-nodes")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renamed.name, "nodes-second");
    assert_eq!(
        renamed.blueprint.fields[1].collection.as_deref(),
        Some("nodes-second")
    );
    let links = crate::store::find_collection(&conn, "second-links")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        links.blueprint.fields[0].collection.as_deref(),
        Some("nodes-second")
    );
    assert_eq!(c.name, "nodes");
    assert_eq!(c.blueprint.fields[1].collection.as_deref(), Some("nodes"));
    assert!(
        conn.query("SELECT COUNT(*) FROM auth_sessions", &[])
            .await
            .is_ok()
    );
    assert!(
        conn.query("SELECT COUNT(*) FROM sessions", &[])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn date_fields_validate_filter_sort_calculate_and_import() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "dates@example.com", "dates").await;
    let tasks = create_collection(
        &app,
        &cookie,
        "tasks",
        "public",
        json!({"title_field":"due","fields":[
            {"name":"title","type":"string"},
            {"name":"due","type":"date"},
            {"name":"due_year","type":"calculated","formula":"=YEAR([due])"}
        ]}),
    )
    .await;
    let documents = format!("/api/v1/collections/{}/documents", tasks.id);
    for (title, due) in [
        ("late", "2026-03-01"),
        ("early", "2025-12-31"),
        ("leap", "2024-02-29"),
    ] {
        let made =
            create_document(&app, &cookie, &tasks.id, json!({"title":title,"due":due})).await;
        assert_eq!(made.body["due"], due);
    }
    let undated = create_document(&app, &cookie, &tasks.id, json!({"title":"undated"})).await;
    assert!(undated.body.get("due").is_none_or(Value::is_null));
    for bad in [
        json!("2025-02-29"),
        json!("31.12.2025"),
        json!("2025-12-31T10:00:00"),
        json!(45000),
    ] {
        assert_eq!(
            app.call(
                "POST",
                &documents,
                Some(&cookie),
                None,
                Some(json!({"body":{"title":"bad","due":bad}}))
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }

    // Range filters compare calendar order and skip records without a date.
    let query = |filters: Value, descending: bool| {
        let app = &app;
        let cookie = cookie.clone();
        let endpoint = format!("/api/v1/collections/{}/query", tasks.id);
        async move {
            let response = app
                .call(
                    "POST",
                    &endpoint,
                    Some(&cookie),
                    None,
                    Some(json!({"filters":filters,"sort_field":"due","descending":descending})),
                )
                .await;
            let status = response.status();
            if status != StatusCode::OK {
                return (status, vec![]);
            }
            let (_, page) = json_of::<vulse_model::RecordPage>(response).await;
            let titles = page
                .items
                .iter()
                .map(|d| d.body["title"].as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            (status, titles)
        }
    };
    assert_eq!(
        query(
            json!([{"field":"due","op":"gte","value":"2025-01-01"}]),
            false
        )
        .await
        .1,
        vec!["early", "late"]
    );
    assert_eq!(
        query(
            json!([{"field":"due","op":"lt","value":"2026-03-01"}]),
            true
        )
        .await
        .1,
        vec!["early", "leap"]
    );
    assert_eq!(
        query(
            json!([{"field":"due","op":"eq","value":"2024-02-29"}]),
            false
        )
        .await
        .1,
        vec!["leap"]
    );
    assert_eq!(
        query(json!([{"field":"due","op":"not_empty","value":""}]), false)
            .await
            .1,
        vec!["leap", "early", "late"]
    );
    for bad in [json!("2025"), json!(2025), json!("2025-02-30")] {
        assert_eq!(
            query(json!([{"field":"due","op":"gt","value":bad}]), false)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }

    // Formulas read the stored ISO text; public reads return it unchanged.
    let (_, public) = json_of::<Page<DocumentView>>(
        app.call("GET", "/api/v1/public/tasks?limit=10", None, None, None)
            .await,
    )
    .await;
    let leap = public
        .items
        .iter()
        .find(|d| d.body["title"] == "leap")
        .unwrap();
    assert_eq!(leap.body["due"], "2024-02-29");
    assert_eq!(leap.body["due_year"], 2024);

    let (status, result) = json_of::<vulse_model::ImportResult>(
        import_csv(
            &app,
            Some(&cookie),
            &tasks.id,
            "title,due\nimported,2027-01-02\n",
            true,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let (_, result) = json_of::<vulse_model::ImportResult>(
        import_csv(
            &app,
            Some(&cookie),
            &tasks.id,
            "title,due\nbad,02/01/2027\n",
            true,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(result.imported, 0);
    assert!(format!("{:?}", result.errors).contains("YYYY-MM-DD"));
}

#[tokio::test]
async fn removing_fields_with_values_needs_confirmation_and_keeps_history() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "fields@example.com", "fields").await;
    let notes = create_collection(
        &app,
        &cookie,
        "notes",
        "private",
        json!({"fields":[
            {"name":"title","type":"string"},
            {"name":"memo","type":"string"},
            {"name":"count","type":"number"}
        ]}),
    )
    .await;
    let kept = create_document(
        &app,
        &cookie,
        &notes.id,
        json!({"title":"A","memo":"keep me","count":1}),
    )
    .await;
    let plain = create_document(&app, &cookie, &notes.id, json!({"title":"B"})).await;
    let collection = format!("/api/v1/collections/{}", notes.id);
    let without_memo =
        json!({"fields":[{"name":"title","type":"string"},{"name":"count","type":"number"}]});

    // Refused by default, naming the field and the records holding values.
    let refused = app
        .call(
            "PATCH",
            &collection,
            Some(&cookie),
            None,
            Some(json!({"blueprint":without_memo})),
        )
        .await;
    let (status, error) = json_of::<ApiErrorBody>(refused).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(error.error.contains("'memo' (1 record)"), "{}", error.error);
    assert_eq!(error.document_ids, Some(vec![kept.id.clone()]));

    // Other incompatible changes explain themselves.
    let (status, error) = json_of::<ApiErrorBody>(
        app.call(
            "PATCH",
            &collection,
            Some(&cookie),
            None,
            Some(json!({"blueprint":{"fields":[{"name":"title","type":"string"},{"name":"memo","type":"string"},{"name":"count","type":"string"}]}})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        error.error.contains("count must be a string"),
        "{}",
        error.error
    );

    let (status, saved) = json_of::<CollectionView>(
        app.call(
            "PATCH",
            &collection,
            Some(&cookie),
            None,
            Some(json!({"blueprint":without_memo,"remove_field_data":true})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved.blueprint.fields.len(), 2);
    let (_, doc) = json_of::<DocumentView>(
        app.call(
            "GET",
            &format!("{collection}/documents/{}", kept.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert!(doc.body.get("memo").is_none());
    assert_eq!(doc.body["count"], 1);
    assert_ne!(doc.updated_at, kept.updated_at);
    let (_, untouched) = json_of::<DocumentView>(
        app.call(
            "GET",
            &format!("{collection}/documents/{}", plain.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(untouched.updated_at, plain.updated_at);

    let (_, revisions) = json_of::<Page<vulse_model::DocumentRevision>>(
        app.call(
            "GET",
            &format!("{collection}/documents/{}/revisions", kept.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(revisions.items[0].action, "fields removed");
    assert!(
        revisions.items[1..]
            .iter()
            .any(|r| r.body["memo"] == "keep me")
    );
    // The search index no longer finds the removed value.
    let (_, page) = json_of::<vulse_model::RecordPage>(
        app.call(
            "POST",
            &format!("{collection}/query"),
            Some(&cookie),
            None,
            Some(json!({"search":"keep me"})),
        )
        .await,
    )
    .await;
    assert_eq!(page.total, 0);
}

#[tokio::test]
async fn renaming_fields_moves_values_formulas_titles_and_views() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "rename@example.com", "rename").await;
    let items = create_collection(
        &app,
        &cookie,
        "items",
        "private",
        json!({"title_field":"name","fields":[
            {"name":"name","type":"string"},
            {"name":"price","type":"number"},
            {"name":"memo","type":"string"},
            {"name":"double","type":"calculated","formula":"=[price]*2"}
        ]}),
    )
    .await;
    let collection = format!("/api/v1/collections/{}", items.id);
    let doc = create_document(
        &app,
        &cookie,
        &items.id,
        json!({"name":"Saw","price":10,"memo":"m"}),
    )
    .await;
    let (status, _) = json_of::<vulse_model::SavedView>(
        app.call(
            "POST",
            &format!("{collection}/views"),
            Some(&cookie),
            None,
            Some(json!({"name":"Pricey","query":{"filters":[{"field":"price","op":"gt","value":5}],"sort_field":"price","group_field":"name"},"columns":["name","price"]})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // A rename needs no data-removal confirmation; a removal in the same save still does.
    let renamed = json!({"title_field":"title","fields":[
        {"name":"title","type":"string","renamed_from":"name"},
        {"name":"unit_price","type":"number","renamed_from":"price"},
        {"name":"double","type":"calculated","formula":"=[price]*2"}
    ]});
    assert_eq!(
        app.call(
            "PATCH",
            &collection,
            Some(&cookie),
            None,
            Some(json!({"blueprint":renamed}))
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let (status, saved) = json_of::<CollectionView>(
        app.call(
            "PATCH",
            &collection,
            Some(&cookie),
            None,
            Some(json!({"blueprint":renamed,"remove_field_data":true})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        saved.blueprint.fields[2].formula.as_deref(),
        Some("=[unit_price]*2")
    );
    assert_eq!(saved.blueprint.title_field.as_deref(), Some("title"));
    assert!(
        saved
            .blueprint
            .fields
            .iter()
            .all(|f| f.renamed_from.is_none())
    );
    let raw = serde_json::to_string(&saved).unwrap();
    assert!(!raw.contains("renamed_from"));

    let (_, moved) = json_of::<DocumentView>(
        app.call(
            "GET",
            &format!("{collection}/documents/{}", doc.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(moved.body["title"], "Saw");
    assert_eq!(moved.body["unit_price"], 10);
    assert_eq!(moved.body["double"].as_f64(), Some(20.0));
    assert!(moved.body.get("price").is_none() && moved.body.get("memo").is_none());

    let (_, views) = json_of::<Vec<vulse_model::SavedView>>(
        app.call(
            "GET",
            &format!("{collection}/views"),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    let view = &views[0].config;
    assert_eq!(view.query.filters[0].field, "unit_price");
    assert_eq!(view.query.sort_field, "unit_price");
    assert_eq!(view.query.group_field, "title");
    assert_eq!(view.columns, vec!["title", "unit_price"]);
    let (status, page) = json_of::<vulse_model::RecordPage>(
        app.call(
            "POST",
            &format!("{collection}/query"),
            Some(&cookie),
            None,
            Some(json!(view.query)),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page.total, 1);

    let (_, revisions) = json_of::<Page<vulse_model::DocumentRevision>>(
        app.call(
            "GET",
            &format!("{collection}/documents/{}/revisions", doc.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(revisions.items[0].action, "fields renamed");

    // Invalid renames are refused before anything changes.
    for bad in [
        json!({"fields":[{"name":"x","type":"string","renamed_from":"missing"}]}),
        json!({"fields":[{"name":"title","type":"string"},{"name":"copy","type":"string","renamed_from":"title"}]}),
    ] {
        assert_eq!(
            app.call(
                "PATCH",
                &collection,
                Some(&cookie),
                None,
                Some(json!({"blueprint":bad,"remove_field_data":true}))
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
}

#[tokio::test]
async fn exports_include_columns_from_related_records() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "related@example.com", "related").await;
    let products = create_collection(
        &app,
        &cookie,
        "products",
        "private",
        json!({"fields":[{"name":"name","type":"string"},{"name":"price","type":"number"},
            {"name":"double","type":"calculated","formula":"=[price]*2"}]}),
    )
    .await;
    let saw = create_document(
        &app,
        &cookie,
        &products.id,
        json!({"name":"Saw","price":10}),
    )
    .await;
    let pen = create_document(
        &app,
        &cookie,
        &products.id,
        json!({"name":"=Pen","price":2}),
    )
    .await;
    let orders = create_collection(
        &app,
        &cookie,
        "orders",
        "private",
        json!({"fields":[{"name":"code","type":"string"},
            {"name":"product","type":"reference","collection":"products"},
            {"name":"extras","type":"array","items":"reference","collection":"products"}]}),
    )
    .await;
    create_document(
        &app,
        &cookie,
        &orders.id,
        json!({"code":"A1","product":saw.id,"extras":[pen.id,saw.id]}),
    )
    .await;
    create_document(&app, &cookie, &orders.id, json!({"code":"B2"})).await;
    let export = format!("/api/v1/collections/{}/export", orders.id);
    let body = |response: axum::response::Response| async move {
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    };

    let (status, csv) = body(
        app.call(
            "POST",
            &export,
            Some(&cookie),
            None,
            Some(json!({"format":"csv","columns":["code","product"],"query":{"sort_field":"code","descending":false},
                "related":[{"field":"product","columns":["name","double"]},{"field":"extras","columns":["name"]}]})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{csv}");
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(
        lines[0],
        "code,product,product.name,product.double,extras.name"
    );
    assert_eq!(
        lines[1],
        format!("A1,{},Saw,20.0,\"[\"\"=Pen\"\",\"\"Saw\"\"]\"", saw.id)
    );
    assert_eq!(lines[2], "B2,,,,[]");

    let (status, json) = body(
        app.call(
            "POST",
            &export,
            Some(&cookie),
            None,
            Some(json!({"format":"json","columns":["code"],"query":{"sort_field":"code","descending":false},
                "related":[{"field":"product","columns":["price"]},{"field":"extras","columns":["name"]}]})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let rows: Vec<Value> = serde_json::from_str(&json).unwrap();
    assert_eq!(rows[0]["body"], json!({"code":"A1"}));
    assert_eq!(
        rows[0]["related"]["product"],
        json!({"id":saw.id,"price":10})
    );
    assert_eq!(
        rows[0]["related"]["extras"],
        json!([{"id":pen.id,"name":"=Pen"},{"id":saw.id,"name":"Saw"}])
    );
    assert_eq!(rows[1]["related"], json!({"product":null,"extras":[]}));

    // Plain exports keep their shape.
    let (_, plain) = body(
        app.call(
            "POST",
            &export,
            Some(&cookie),
            None,
            Some(json!({"format":"json","columns":["code"]})),
        )
        .await,
    )
    .await;
    assert!(!plain.contains("related"));

    for (related, expected) in [
        (
            json!([{"field":"code","columns":["name"]}]),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!([{"field":"product","columns":["missing"]}]),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!([{"field":"product","columns":[]}]),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!([{"field":"product","columns":["name"]},{"field":"product","columns":["price"]}]),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            app.call(
                "POST",
                &export,
                Some(&cookie),
                None,
                Some(json!({"format":"csv","columns":["code"],"related":related}))
            )
            .await
            .status(),
            expected,
            "{related}"
        );
    }

    // A key scoped to orders cannot read products through a link.
    let (_, key) = json_of::<CreatedApiKey>(
        app.call(
            "POST",
            "/api/v1/api-keys",
            Some(&cookie),
            None,
            Some(json!({"name":"orders only","collection_id":orders.id})),
        )
        .await,
    )
    .await;
    let scoped = |related: Value| {
        let app = &app;
        let export = export.clone();
        let secret = key.secret.clone();
        async move {
            app.call(
                "POST",
                &export,
                None,
                Some(&secret),
                Some(json!({"format":"csv","columns":["code"],"related":related})),
            )
            .await
            .status()
        }
    };
    assert_eq!(scoped(json!([])).await, StatusCode::OK);
    assert_eq!(
        scoped(json!([{"field":"product","columns":["name"]}])).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn summaries_group_aggregate_calculate_and_save_with_views() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "summary@example.com", "summary").await;
    let regions = create_collection(
        &app,
        &cookie,
        "regions",
        "private",
        json!({"fields":[{"name":"title","type":"string"}]}),
    )
    .await;
    let north = create_document(&app, &cookie, &regions.id, json!({"title":"North"})).await;
    let kids = create_collection(
        &app,
        &cookie,
        "kindergartens",
        "public",
        json!({"fields":[
            {"name":"name","type":"string"},
            {"name":"revenue","type":"number"},
            {"name":"employees","type":"number"},
            {"name":"opened","type":"date"},
            {"name":"region","type":"reference","collection":"regions"},
            {"name":"per_employee","type":"calculated","formula":"=[revenue]/[employees]"}
        ]}),
    )
    .await;
    for body in [
        json!({"name":"Barnehage1","revenue":1000,"employees":4,"opened":"2026-01-15","region":north.id}),
        json!({"name":"Barnehage1","revenue":2000,"employees":5,"opened":"2026-01-20","region":north.id}),
        json!({"name":"Barnehage2","revenue":4000,"employees":10,"opened":"2026-03-02"}),
        json!({"name":"barnehage1","revenue":0.5,"employees":1}),
        json!({"revenue":7,"employees":1}),
    ] {
        create_document(&app, &cookie, &kids.id, body).await;
    }
    let endpoint = format!("/api/v1/collections/{}/summary", kids.id);
    let summarize = |body: Value| {
        let app = &app;
        let cookie = cookie.clone();
        let endpoint = endpoint.clone();
        async move {
            let response = app
                .call("POST", &endpoint, Some(&cookie), None, Some(body))
                .await;
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    let spec = json!({
        "group_by":[{"field":"name"}],
        "measures":[
            {"name":"total_revenue","op":"sum","field":"revenue"},
            {"name":"average_employees","op":"average","field":"employees"},
            {"name":"records","op":"count"},
            {"name":"scaled","op":"formula","formula":"=[total_revenue]/3.14"},
            {"name":"combined","op":"formula","formula":"=[total_revenue]*[average_employees]"}
        ]
    });
    let (status, result) = summarize(json!({"summary":spec})).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let rows = result["rows"].as_array().unwrap();
    // Case is kept, so "barnehage1" is its own group; records without a name come last.
    let names: Vec<_> = rows.iter().map(|r| r["group"][0].clone()).collect();
    assert_eq!(
        names,
        vec![
            json!("Barnehage1"),
            json!("barnehage1"),
            json!("Barnehage2"),
            Value::Null
        ]
    );
    assert_eq!(rows[0]["values"]["total_revenue"], 3000);
    assert_eq!(rows[0]["values"]["average_employees"], 4.5);
    assert_eq!(rows[0]["values"]["records"], 2);
    assert_eq!(rows[0]["values"]["combined"].as_f64(), Some(13500.0));
    assert!((rows[0]["values"]["scaled"].as_f64().unwrap() - 955.414).abs() < 0.001);
    assert_eq!(rows[2]["values"]["total_revenue"], 4000);
    assert_eq!(rows[2]["values"]["average_employees"], 10);
    assert_eq!(rows[3]["labels"][0], "(empty)");
    assert_eq!(result["totals"]["count"], 5);
    assert_eq!(result["totals"]["values"]["total_revenue"], 7007.5);
    assert_eq!(result["group_count"], 4);
    assert_eq!(result["truncated"], false);

    // Filters apply first; sorting by a value; date buckets; reference titles;
    // calculated fields can be aggregated; dates have min and max.
    let (status, result) = summarize(json!({
        "query":{"filters":[{"field":"revenue","op":"gte","value":1000}]},
        "summary":{
            "group_by":[{"field":"opened","bucket":"month"},{"field":"region"}],
            "measures":[
                {"name":"revenue","op":"sum","field":"revenue"},
                {"name":"best_ratio","op":"max","field":"per_employee"},
                {"name":"first","op":"min","field":"opened"},
                {"name":"names","op":"count_unique","field":"name"}
            ],
            "sort":"revenue","descending":true
        }
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let rows = result["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["group"], json!(["2026-03", null]));
    assert_eq!(rows[0]["labels"], json!(["2026-03", "(empty)"]));
    assert_eq!(rows[1]["group"], json!(["2026-01", north.id]));
    assert_eq!(rows[1]["labels"], json!(["2026-01", "North"]));
    assert_eq!(
        rows[1]["values"],
        json!({"revenue":3000,"best_ratio":400,"first":"2026-01-15","names":1})
    );

    let (status, error) =
        summarize(json!({"summary":{"measures":[{"name":"t","op":"sum","field":"name"}]}})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("sum cannot be used")
    );

    // Saved with a view, runnable by id, and readable by viewers.
    let views = format!("/api/v1/collections/{}/views", kids.id);
    let (status, view) = json_of::<vulse_model::SavedView>(
        app.call(
            "POST",
            &views,
            Some(&cookie),
            None,
            Some(
                json!({"name":"Per kindergarten","query":{},"columns":["name"],"summary":{
                    "group_by":[{"field":"name"}],
                    "measures":[{"name":"total_revenue","op":"sum","field":"revenue"}],
                    "chart":{"kind":"bar","measures":["total_revenue"]}
                }}),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (viewer, _) = register_as(&app, "sv@example.com", "sv", Role::Viewer).await;
    let (status, saved) = json_of::<vulse_model::SummaryResult>(
        app.call(
            "GET",
            &format!("{views}/{}/summary", view.id),
            Some(&viewer),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved.rows[0].values["total_revenue"], 3000);
    assert_eq!(
        app.call(
            "POST",
            &views,
            Some(&cookie),
            None,
            Some(json!({"name":"Bad","query":{},"columns":["name"],"summary":{"measures":[{"name":"x","op":"sum","field":"nope"}]}}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    // Renaming a field keeps saved summaries working.
    let mut blueprint = kids.blueprint.clone();
    blueprint.fields[0].name = "school".into();
    blueprint.fields[0].renamed_from = Some("name".into());
    assert_eq!(
        app.call(
            "PATCH",
            &format!("/api/v1/collections/{}", kids.id),
            Some(&cookie),
            None,
            Some(json!({"blueprint":blueprint}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let (status, renamed) = json_of::<vulse_model::SummaryResult>(
        app.call(
            "GET",
            &format!("{views}/{}/summary", view.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed.rows[0].group[0], "Barnehage1");

    // Public collections can be summarized without credentials, even cross-site.
    let public_spec = json!({"summary":{"measures":[{"name":"n","op":"count"}]}});
    let response = app
        .call_with(
            "POST",
            "/api/v1/public/kindergartens/summary",
            None,
            None,
            Some(public_spec.clone()),
            &[
                ("sec-fetch-site", "cross-site"),
                ("origin", "https://site.example"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .unwrap(),
        "*"
    );
    assert_eq!(
        app.call(
            "POST",
            "/api/v1/public/regions/summary",
            None,
            None,
            Some(public_spec)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn changing_field_types_converts_stored_values_on_request() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "convert@example.com", "convert").await;
    let items = create_collection(
        &app,
        &cookie,
        "items",
        "private",
        json!({"fields":[{"name":"title","type":"string"},{"name":"amount","type":"string"},{"name":"active","type":"string"}]}),
    )
    .await;
    let collection = format!("/api/v1/collections/{}", items.id);
    let a = create_document(
        &app,
        &cookie,
        &items.id,
        json!({"title":"A","amount":"12","active":"yes"}),
    )
    .await;
    let b = create_document(
        &app,
        &cookie,
        &items.id,
        json!({"title":"B","amount":" 3.5 ","active":"no"}),
    )
    .await;
    let c = create_document(&app, &cookie, &items.id, json!({"title":"C","amount":""})).await;
    let d = create_document(&app, &cookie, &items.id, json!({"title":"D"})).await;
    let numbers = json!({"fields":[{"name":"title","type":"string"},{"name":"amount","type":"number"},{"name":"active","type":"boolean"}]});
    let patch = |body: Value| {
        let app = &app;
        let collection = collection.clone();
        let cookie = cookie.clone();
        async move {
            let response = app
                .call("PATCH", &collection, Some(&cookie), None, Some(body))
                .await;
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    let read = |id: String| {
        let app = &app;
        let collection = collection.clone();
        let cookie = cookie.clone();
        async move {
            json_of::<DocumentView>(
                app.call(
                    "GET",
                    &format!("{collection}/documents/{id}"),
                    Some(&cookie),
                    None,
                    None,
                )
                .await,
            )
            .await
            .1
        }
    };

    // Without confirmation the error explains how to convert.
    let (status, error) = patch(json!({"blueprint":numbers})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let message = error["error"].as_str().unwrap();
    assert!(
        message.contains("convert_field_values") && message.contains("'amount' (text → a number)"),
        "{message}"
    );

    // A value that cannot be converted stops everything and is named.
    let bad = create_document(
        &app,
        &cookie,
        &items.id,
        json!({"title":"E","amount":"12a"}),
    )
    .await;
    let (status, error) = patch(json!({"blueprint":numbers,"convert_field_values":true})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let message = error["error"].as_str().unwrap();
    assert!(
        message.starts_with("1 record cannot be converted") && message.contains("“12a”"),
        "{message}"
    );
    assert_eq!(error["document_ids"], json!([bad.id]));
    assert_eq!(read(a.id.clone()).await.body["amount"], "12");

    // After fixing it, the conversion runs.
    assert_eq!(
        app.call(
            "PATCH",
            &format!("{collection}/documents/{}", bad.id),
            Some(&cookie),
            None,
            Some(json!({"patch":{"amount":"7"}}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let (status, saved) = patch(json!({"blueprint":numbers,"convert_field_values":true})).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let converted = read(a.id.clone()).await;
    assert_eq!(
        (
            converted.body["amount"].clone(),
            converted.body["active"].clone()
        ),
        (json!(12), json!(true))
    );
    let converted = read(b.id.clone()).await;
    assert_eq!(
        (
            converted.body["amount"].clone(),
            converted.body["active"].clone()
        ),
        (json!(3.5), json!(false))
    );
    assert!(read(c.id.clone()).await.body.get("amount").is_none());
    let untouched = read(d.id.clone()).await;
    assert_eq!(untouched.updated_at, d.updated_at);

    let (_, revisions) = json_of::<Page<vulse_model::DocumentRevision>>(
        app.call(
            "GET",
            &format!("{collection}/documents/{}/revisions", a.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(revisions.items[0].action, "fields converted");
    assert!(
        revisions.items[1..]
            .iter()
            .any(|r| r.body["amount"] == "12")
    );

    // Numbers now sum, and a renamed field converts too.
    let (_, summary) = json_of::<vulse_model::SummaryResult>(
        app.call(
            "POST",
            &format!("{collection}/summary"),
            Some(&cookie),
            None,
            Some(json!({"summary":{"measures":[{"name":"total","op":"sum","field":"amount"}]}})),
        )
        .await,
    )
    .await;
    assert_eq!(summary.totals.values["total"], 22.5);
    let (status, _) = patch(json!({"blueprint":{"fields":[
        {"name":"title","type":"string"},
        {"name":"label","type":"string","renamed_from":"amount"},
        {"name":"active","type":"boolean"}
    ]},"convert_field_values":true}))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read(b.id.clone()).await.body["label"], "3.5");
}

#[tokio::test]
async fn formulas_read_columns_of_other_collections() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "lookups@example.com", "lookups").await;
    let reports = create_collection(
        &app,
        &cookie,
        "basil",
        "private",
        json!({"fields":[{"name":"ar","type":"number"},{"name":"org_nr","type":"string"},{"name":"arsverk","type":"number"},{"name":"double","type":"calculated","formula":"=[arsverk]*2"}]}),
    )
    .await;
    let weighted = "=IFERROR(SUMIFS([basil.arsverk];[basil.org_nr];[org_nr];[basil.ar];[ar]-2);\"\")*7/12+IFERROR(SUMIFS([basil.arsverk];[basil.org_nr];[org_nr];[basil.ar];[ar]-1);\"\")*5/12";
    let send = |method: &'static str, path: String, body: Option<Value>| {
        let app = &app;
        let cookie = cookie.clone();
        async move {
            let response = app.call(method, &path, Some(&cookie), None, body).await;
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let post_collection = |blueprint: Value| {
        send(
            "POST",
            "/api/v1/collections".into(),
            Some(json!({"name":"pensjon","visibility":"private","blueprint":blueprint})),
        )
    };
    // References must name existing, non-chaining fields.
    for (formula, message) in [
        ("=SUM([nowhere.amount])", "unknown collection"),
        ("=SUM([basil.missing])", "has no field"),
    ] {
        let (status, body) =
            post_collection(json!({"fields":[{"name":"x","type":"calculated","formula":formula}]}))
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains(message), "{body}");
    }
    let pension = create_collection(
        &app,
        &cookie,
        "pensjon",
        "private",
        json!({"fields":[
            {"name":"org_nr","type":"string"},
            {"name":"ar","type":"number"},
            {"name":"weighted","type":"calculated","formula":weighted},
            {"name":"doubled","type":"calculated","formula":"=SUMIFS([basil.double],[basil.org_nr],[org_nr])"}
        ]}),
    )
    .await;
    let (status, body) = send(
        "POST",
        "/api/v1/collections".into(),
        Some(json!({"name":"chained","visibility":"private","blueprint":{"fields":[{"name":"x","type":"calculated","formula":"=SUM([pensjon.weighted])"}]}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("reads other collections itself"),
        "{body}"
    );

    let first = create_document(
        &app,
        &cookie,
        &reports.id,
        json!({"ar":2022,"org_nr":"920143067","arsverk":3}),
    )
    .await;
    create_document(
        &app,
        &cookie,
        &reports.id,
        json!({"ar":2023,"org_nr":"920143067","arsverk":3.55}),
    )
    .await;
    create_document(
        &app,
        &cookie,
        &reports.id,
        json!({"ar":2023,"org_nr":"920062407","arsverk":27.55}),
    )
    .await;
    let record = create_document(
        &app,
        &cookie,
        &pension.id,
        json!({"org_nr":"920143067","ar":2024}),
    )
    .await;
    let expected = 3.0 * 7.0 / 12.0 + 3.55 * 5.0 / 12.0;
    assert!(
        (record.body["weighted"].as_f64().unwrap() - expected).abs() < 1e-9,
        "{:?}",
        record
    );
    assert_eq!(record.body["doubled"].as_f64(), Some(13.1));

    // Reads see changes to the other collection right away.
    let (status, _) = send(
        "PUT",
        format!("/api/v1/collections/{}/documents/{}", reports.id, first.id),
        Some(json!({"body":{"ar":2022,"org_nr":"920143067","arsverk":6}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let read = |id: String| {
        send(
            "GET",
            format!("/api/v1/collections/{}/documents/{id}", pension.id),
            None,
        )
    };
    let (_, body) = read(record.id.clone()).await;
    let expected = 6.0 * 7.0 / 12.0 + 3.55 * 5.0 / 12.0;
    assert!(
        (body["body"]["weighted"].as_f64().unwrap() - expected).abs() < 1e-9,
        "{body}"
    );

    // Queries list the calculated values too, and saving filters on them.
    let (status, page) = send(
        "POST",
        format!("/api/v1/collections/{}/query", pension.id),
        Some(json!({"filters":[{"field":"doubled","op":"gt","value":10}]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["total"], json!(1), "{page}");

    // Renaming the source collection and its fields rewrites the formulas.
    let (status, body) = send(
        "PATCH",
        format!("/api/v1/collections/{}", reports.id),
        Some(json!({"name":"basil-arsmelding","blueprint":{"fields":[{"name":"ar","type":"number"},{"name":"org_nr","type":"string"},{"name":"fte","renamed_from":"arsverk","type":"number"},{"name":"double","type":"calculated","formula":"=[fte]*2"}]}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = send("GET", format!("/api/v1/collections/{}", pension.id), None).await;
    let formula = body["blueprint"]["fields"][2]["formula"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        formula.contains("[basil-arsmelding.fte]") && !formula.contains("[basil."),
        "{formula}"
    );
    let (_, body) = read(record.id.clone()).await;
    assert!(
        (body["body"]["weighted"].as_f64().unwrap() - expected).abs() < 1e-9,
        "{body}"
    );

    // Fields and collections that formulas read cannot disappear.
    let (status, body) = send(
        "PATCH",
        format!("/api/v1/collections/{}", reports.id),
        Some(json!({"remove_field_data":true,"blueprint":{"fields":[{"name":"ar","type":"number"},{"name":"org_nr","type":"string"},{"name":"double","type":"calculated","formula":"=2"}]}})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("pensjon.weighted"),
        "{body}"
    );
    let (status, _) = send(
        "DELETE",
        format!("/api/v1/collections/{}", reports.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Nor may a field that is read start reading other collections itself.
    let (status, body) = send(
        "PATCH",
        format!("/api/v1/collections/{}", reports.id),
        Some(json!({"blueprint":{"fields":[{"name":"ar","type":"number"},{"name":"org_nr","type":"string"},{"name":"fte","type":"number"},{"name":"double","type":"calculated","formula":"=COUNTIFS([pensjon.ar],[ar])"}]}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn views_hear_about_changes_to_their_collection_and_what_it_shows() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "events@example.com", "events").await;
    let source = create_collection(
        &app,
        &cookie,
        "reports",
        "private",
        json!({"fields":[{"name":"org","type":"string"},{"name":"fte","type":"number"}]}),
    )
    .await;
    let unrelated = create_collection(
        &app,
        &cookie,
        "unrelated",
        "private",
        json!({"fields":[{"name":"x","type":"number"}]}),
    )
    .await;
    let view = create_collection(
        &app,
        &cookie,
        "kids",
        "private",
        json!({"fields":[{"name":"org","type":"string"},{"name":"fte","type":"calculated","formula":"=SUMIFS([reports.fte],[reports.org],[org])"}]}),
    )
    .await;
    let response = app
        .call(
            "GET",
            &format!("/api/v1/collections/{}/events", view.id),
            Some(&cookie),
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut body = response.into_body();
    let mut next_event = async || -> String {
        loop {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
                .await
                .expect("an event within 5 seconds")
                .unwrap()
                .unwrap();
            if let Ok(data) = frame.into_data() {
                let text = String::from_utf8(data.to_vec()).unwrap();
                if text.starts_with("event: change") {
                    return text;
                }
            }
        }
    };
    // Unrelated writes are skipped; the formula's source collection counts.
    create_document(&app, &cookie, &unrelated.id, json!({"x":1})).await;
    create_document(&app, &cookie, &source.id, json!({"org":"a","fte":2})).await;
    assert!(next_event().await.contains(&source.id));
    create_document(&app, &cookie, &unrelated.id, json!({"x":2})).await;
    let doc = create_document(&app, &cookie, &view.id, json!({"org":"a"})).await;
    assert!(next_event().await.contains(&view.id));
    let response = app
        .call(
            "DELETE",
            &format!("/api/v1/collections/{}/documents/{}", view.id, doc.id),
            Some(&cookie),
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(next_event().await.contains(&view.id));
    // Signed-out requests are refused like any read.
    let response = app
        .call(
            "GET",
            &format!("/api/v1/collections/{}/events", view.id),
            None,
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn allowed_values_are_enforced_and_renames_move_records_and_views() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "options@example.com", "options").await;
    let field = |options: Value| json!({"name":"status","type":"string","options":options});
    // Allowed values are checked when fields are saved.
    for (bad, message) in [
        (json!([]), "1 to 500"),
        (json!(["Ja", "ja"]), "more than once"),
        (json!([" Ja"]), "spaces"),
    ] {
        let response = app
            .call(
                "POST",
                "/api/v1/collections",
                Some(&cookie),
                None,
                Some(json!({"name":"bad","visibility":"private","blueprint":{"fields":[field(bad)]}})),
            )
            .await;
        let (status, error) = json_of::<ApiErrorBody>(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error.error.contains(message), "{}", error.error);
    }
    let response = app
        .call(
            "POST",
            "/api/v1/collections",
            Some(&cookie),
            None,
            Some(json!({"name":"bad","visibility":"private","blueprint":{"fields":[{"name":"n","type":"number","options":["1"]}]}})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let municipalities = create_collection(
        &app,
        &cookie,
        "municipalities",
        "private",
        json!({"fields":[{"name":"name","type":"string"},field(json!(["Funded","Underfunded"]))]}),
    )
    .await;
    let collection = format!("/api/v1/collections/{}", municipalities.id);
    let oslo = create_document(
        &app,
        &cookie,
        &municipalities.id,
        json!({"name":"Oslo","status":"Underfunded"}),
    )
    .await;
    create_document(
        &app,
        &cookie,
        &municipalities.id,
        json!({"name":"Bergen","status":"Funded"}),
    )
    .await;
    create_document(&app, &cookie, &municipalities.id, json!({"name":"Moss"})).await;
    // Values must match exactly; leaving an optional value out is fine.
    for bad in ["underfunded", "Other", ""] {
        let response = app
            .call(
                "POST",
                &format!("{collection}/documents"),
                Some(&cookie),
                None,
                Some(json!({"body":{"name":"X","status":bad}})),
            )
            .await;
        let (status, error) = json_of::<ApiErrorBody>(response).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error.error.contains("allowed values"), "{}", error.error);
    }
    let (status, _) = json_of::<vulse_model::SavedView>(
        app.call(
            "POST",
            &format!("{collection}/views"),
            Some(&cookie),
            None,
            Some(json!({"name":"Needs funding","query":{"filters":[{"field":"status","op":"eq","value":"Underfunded"}]},"columns":["name","status"]})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let patch = |blueprint: Value| {
        let app = &app;
        let cookie = cookie.clone();
        let collection = collection.clone();
        async move {
            let response = app
                .call(
                    "PATCH",
                    &collection,
                    Some(&cookie),
                    None,
                    Some(json!({"blueprint":blueprint})),
                )
                .await;
            json_of::<Value>(response).await
        }
    };
    // Removing a value that records hold is refused with those records.
    let (status, error) =
        patch(json!({"fields":[{"name":"name","type":"string"},field(json!(["Funded"]))]})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["document_ids"], json!([oslo.id]));
    // Renames must name saved values and new allowed values.
    for renamed in [
        json!({"Missing":"Funded"}),
        json!({"Underfunded":"Elsewhere"}),
        json!({"Underfunded":"Funded","Funded":"Fine"}),
    ] {
        let mut status_field = field(json!(["Funded", "Fine"]));
        status_field["renamed_options"] = renamed;
        let (status, _) =
            patch(json!({"fields":[{"name":"name","type":"string"},status_field]})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // Renaming a value, together with the field, moves records and views.
    let (status, saved) = patch(json!({"fields":[
        {"name":"name","type":"string"},
        {"name":"funding","type":"string","renamed_from":"status",
         "options":["Funded","Needs funding"],"renamed_options":{"Underfunded":"Needs funding"}}
    ]}))
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert!(
        saved["blueprint"]["fields"][1]
            .get("renamed_options")
            .is_none()
    );
    let (_, record) = json_of::<DocumentView>(
        app.call(
            "GET",
            &format!("{collection}/documents/{}", oslo.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(
        record.body,
        json!({"name":"Oslo","funding":"Needs funding"})
    );
    let (_, views) = json_of::<Vec<vulse_model::SavedView>>(
        app.call(
            "GET",
            &format!("{collection}/views"),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(views[0].config.query.filters[0].field, "funding");
    assert_eq!(
        views[0].config.query.filters[0].value,
        json!("Needs funding")
    );
    let (_, page) = json_of::<vulse_model::RecordPage>(
        app.call(
            "POST",
            &format!("{collection}/query"),
            Some(&cookie),
            None,
            Some(json!(views[0].config.query)),
        )
        .await,
    )
    .await;
    assert_eq!(page.total, 1);
    let (_, revisions) = json_of::<Page<vulse_model::DocumentRevision>>(
        app.call(
            "GET",
            &format!("{collection}/documents/{}/revisions", oslo.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(revisions.items[0].action, "fields renamed");

    // A value rename alone; merging two values into one is allowed.
    let (status, _) = patch(json!({"fields":[
        {"name":"name","type":"string"},
        {"name":"funding","type":"string","options":["Status known"],
         "renamed_options":{"Funded":"Status known","Needs funding":"Status known"}}
    ]}))
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, revisions) = json_of::<Page<vulse_model::DocumentRevision>>(
        app.call(
            "GET",
            &format!("{collection}/documents/{}/revisions", oslo.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(revisions.items[0].action, "values renamed");
    assert_eq!(revisions.items[0].body["funding"], json!("Status known"));
    // Dropping the list keeps the values as free text.
    let (status, _) = patch(
        json!({"fields":[{"name":"name","type":"string"},{"name":"funding","type":"string"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn filters_and_sorting_follow_changes_in_collections_formulas_read() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "readers@example.com", "readers").await;
    let reports = create_collection(
        &app,
        &cookie,
        "reports",
        "private",
        json!({"fields":[{"name":"org","type":"string"},{"name":"fte","type":"number"}]}),
    )
    .await;
    let orgs = create_collection(
        &app,
        &cookie,
        "orgs",
        "private",
        json!({"fields":[{"name":"org","type":"string"},{"name":"fte","type":"calculated","formula":"=SUMIFS([reports.fte],[reports.org],[org])"}]}),
    )
    .await;
    // Reads its own collection: each record's share of the total.
    let shares = create_collection(
        &app,
        &cookie,
        "shares",
        "private",
        json!({"fields":[{"name":"name","type":"string"},{"name":"amount","type":"number"},{"name":"share","type":"calculated","formula":"=[amount]/SUM([shares.amount])"}]}),
    )
    .await;
    create_document(&app, &cookie, &orgs.id, json!({"org":"a"})).await;
    create_document(&app, &cookie, &orgs.id, json!({"org":"b"})).await;
    let query = |collection: String, field: &'static str, filters: Value| {
        let app = &app;
        let cookie = cookie.clone();
        async move {
            let response = app
                .call(
                    "POST",
                    &format!("/api/v1/collections/{collection}/query"),
                    Some(&cookie),
                    None,
                    Some(json!({"filters":filters,"sort_field":field,"descending":true})),
                )
                .await;
            let (status, page) = json_of::<vulse_model::RecordPage>(response).await;
            assert_eq!(status, StatusCode::OK);
            page.items
                .iter()
                .map(|d| d.body[if field == "fte" { "org" } else { "name" }].clone())
                .collect::<Vec<_>>()
        }
    };
    let busy = || json!([{"field":"fte","op":"gte","value":5}]);

    // The orgs were saved before any reports existed.
    create_document(&app, &cookie, &reports.id, json!({"org":"a","fte":3})).await;
    let b = create_document(&app, &cookie, &reports.id, json!({"org":"b","fte":6})).await;
    assert_eq!(query(orgs.id.clone(), "fte", busy()).await, [json!("b")]);
    assert_eq!(
        query(orgs.id.clone(), "fte", json!([])).await,
        [json!("b"), json!("a")]
    );

    let response = app
        .call(
            "PUT",
            &format!("/api/v1/collections/{}/documents/{}", reports.id, b.id),
            Some(&cookie),
            None,
            Some(json!({"body":{"org":"b","fte":1}})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        query(orgs.id.clone(), "fte", busy()).await,
        Vec::<Value>::new()
    );
    assert_eq!(
        query(orgs.id.clone(), "fte", json!([])).await,
        [json!("a"), json!("b")]
    );

    create_document(&app, &cookie, &reports.id, json!({"org":"b","fte":5})).await;
    let response = app
        .call(
            "DELETE",
            &format!("/api/v1/collections/{}/documents/{}", reports.id, b.id),
            Some(&cookie),
            None,
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(query(orgs.id.clone(), "fte", busy()).await, [json!("b")]);

    // A new record changes the share of the records saved before it.
    create_document(&app, &cookie, &shares.id, json!({"name":"x","amount":1})).await;
    create_document(&app, &cookie, &shares.id, json!({"name":"y","amount":3})).await;
    let half = || json!([{"field":"share","op":"gte","value":0.5}]);
    assert_eq!(
        query(shares.id.clone(), "share", half()).await,
        [json!("y")]
    );
    create_document(&app, &cookie, &shares.id, json!({"name":"z","amount":4})).await;
    assert_eq!(
        query(shares.id.clone(), "share", half()).await,
        [json!("z")]
    );
}

#[tokio::test]
async fn sql_reads_through_the_record_table_match_full_evaluation() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "narrow@example.com", "narrow").await;
    let c = create_collection(
        &app,
        &cookie,
        "things",
        "private",
        json!({"fields":[
            {"name":"name","type":"string","required":true},
            {"name":"city","type":"string"},
            {"name":"score","type":"number"},
            {"name":"code","type":"string"},
            {"name":"due","type":"date"},
            {"name":"flag","type":"boolean"},
            {"name":"tags","type":"array","items":"string"},
            {"name":"calc_n","type":"calculated","formula":"=[score]*2"},
            {"name":"calc_t","type":"calculated","formula":"=UPPER([city])"}
        ]}),
    )
    .await;
    for body in [
        json!({"name":"Alpha","city":"Oslo","score":10,"code":"12","due":"2024-01-05","flag":true,"tags":["a","b"]}),
        json!({"name":"beta","city":"oslo","score":2.5,"code":"007","due":"2023-12-31","flag":false}),
        json!({"name":"Gamma","city":"","score":-1,"code":"abc"}),
        json!({"name":"delta"}),
        json!({"name":"Échelle","city":"Bergen","score":100,"code":"1e2","due":"2025-06-01","tags":[]}),
        json!({"name":"100","city":"Trondheim","score":0,"code":" 5 ","flag":true}),
    ] {
        create_document(&app, &cookie, &c.id, body).await;
    }
    let (_, page) = json_of::<Page<DocumentView>>(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}/documents?limit=100", c.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    let rows: Vec<vulse_model::SqlRow> = page
        .items
        .iter()
        .map(|doc| {
            let mut row = doc.body.as_object().cloned().unwrap();
            row.insert("id".into(), json!(doc.id));
            row.insert("created_at".into(), json!(doc.created_at));
            row.insert("updated_at".into(), json!(doc.updated_at));
            row
        })
        .collect();
    // Values read from the record table equal the record bodies.
    let (_, all) = json_of::<Value>(
        app.call(
            "POST",
            "/api/v1/sql",
            Some(&cookie),
            None,
            Some(json!({"sql":"SELECT * FROM things"})),
        )
        .await,
    )
    .await;
    let columns: Vec<String> = serde_json::from_value(all["columns"].clone()).unwrap();
    assert_eq!(all["rows"].as_array().unwrap().len(), rows.len());
    for found in all["rows"].as_array().unwrap() {
        let row = rows.iter().find(|r| r["id"] == found[0]).unwrap();
        for (index, column) in columns.iter().enumerate() {
            let expected = row.get(column).cloned().unwrap_or(Value::Null);
            let same = match (expected.as_f64(), found[index].as_f64()) {
                (Some(a), Some(b)) => a == b,
                _ => expected == found[index],
            };
            assert!(same, "{column}: {expected} vs {}", found[index]);
        }
    }
    let first = &page.items[0].id;
    let wheres = [
        "score > 5",
        "5 < score",
        "score = 10",
        "score <> 10",
        "score >= 2.5 AND city = 'Oslo'",
        "city = 'oslo'",
        "city = 'Oslo'",
        "city <> 'Oslo'",
        "city IS NULL",
        "city IS NOT NULL",
        "city LIKE 'o%'",
        "name LIKE '_cH%'",
        "name LIKE 'é%'",
        "code = 12",
        "code > 6",
        "code < 'b'",
        "code = '12'",
        "code IN (12, 'abc')",
        "code BETWEEN 5 AND 100",
        "score BETWEEN 0 AND 10",
        "score NOT BETWEEN 0 AND 10",
        "score IN (2.5, 100)",
        "score NOT IN (2.5)",
        "due > '2024-01-01'",
        "due <= '2024-01-05'",
        "due = '2023-12-31'",
        "due > 2000",
        "flag IS NULL",
        "flag IS NOT NULL",
        "tags IS NULL",
        "tags IS NOT NULL",
        "calc_n > 10",
        "calc_n = 20",
        "calc_t = 'OSLO'",
        "calc_t LIKE 'os%'",
        "name = '100'",
        "name = 100",
        "name > 50",
        "created_at > '2000'",
        "updated_at IS NOT NULL",
        "id IS NULL",
        "score > 5 OR city = 'Bergen'",
        "score > 5 OR LENGTH(name) = 4",
        "NOT score > 5",
        "COALESCE(city, 'x') = 'x'",
        "score * 2 > 10",
        "-score < 0",
        "city = 'Oslo' AND (score > 5 OR code = 'abc')",
        "name = 'Échelle'",
        "name = 'échelle'",
        "code = 5",
        "code = 100",
        "score = '10'",
        "'Oslo' = city",
        "10 <= score",
    ];
    let mut checks: Vec<String> = wheres.iter().map(|w| w.to_string()).collect();
    checks.push(format!("id = '{first}'"));
    checks.push(format!("id IN ('{first}', 'missing')"));
    for clause in checks {
        let statement =
            vulse_model::parse_sql(&format!("SELECT id FROM things WHERE {clause}")).unwrap();
        let mut expected: Vec<String> = rows
            .iter()
            .filter(|row| vulse_model::sql_matches(statement.filter(), row).unwrap())
            .map(|row| row["id"].as_str().unwrap().to_string())
            .collect();
        expected.sort();
        let response = app
            .call(
                "POST",
                "/api/v1/sql",
                Some(&cookie),
                None,
                Some(json!({"sql": format!("SELECT id FROM things WHERE {clause}")})),
            )
            .await;
        let (status, body) = json_of::<Value>(response).await;
        assert_eq!(status, StatusCode::OK, "{clause}: {body}");
        let mut found: Vec<String> = body["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[0].as_str().unwrap().to_string())
            .collect();
        found.sort();
        assert_eq!(found, expected, "{clause}");
        // Updates and deletes plan over the same records.
        let response = app
            .call(
                "POST",
                "/api/v1/sql",
                Some(&cookie),
                None,
                Some(json!({"sql": format!("DELETE FROM things WHERE {clause}")})),
            )
            .await;
        let (status, body) = json_of::<Value>(response).await;
        assert_eq!(status, StatusCode::OK, "{clause}: {body}");
        assert_eq!(body["affected"], json!(expected.len()), "{clause}");
    }
}

#[tokio::test]
async fn sql_statements_preview_confirm_and_use_the_record_path() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "sql@example.com", "sql").await;
    let reports = create_collection(
        &app,
        &cookie,
        "aarsmelding",
        "private",
        json!({"fields":[{"name":"aar","type":"number"},{"name":"navn","type":"string","required":true},{"name":"kommune","type":"string"},{"name":"fte","type":"number"},{"name":"double","type":"calculated","formula":"=[fte]*2"}]}),
    )
    .await;
    for (aar, navn, kommune, fte) in [
        (2018, "A", "Oslo", 2.0),
        (2019, "B", "Oslo", 3.0),
        (2023, "C", "Bergen", 4.0),
        (2024, "D", "Oslo", 5.5),
    ] {
        create_document(
            &app,
            &cookie,
            &reports.id,
            json!({"aar":aar,"navn":navn,"kommune":kommune,"fte":fte}),
        )
        .await;
    }
    let sql = |who: &str, body: Value| {
        let app = &app;
        let who = who.to_string();
        async move {
            let response = app
                .call("POST", "/api/v1/sql", Some(&who), None, Some(body))
                .await;
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    // SELECT with grouping, calculated fields and ordering.
    let (status, body) = sql(&cookie, json!({"sql":"SELECT kommune, COUNT(*) AS n, SUM(double) AS total FROM aarsmelding GROUP BY kommune ORDER BY n DESC"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["columns"], json!(["kommune", "n", "total"]));
    assert_eq!(body["rows"], json!([["Oslo", 3, 21], ["Bergen", 1, 8]]));
    // Unknown names and syntax errors explain themselves.
    let (status, body) = sql(&cookie, json!({"sql":"SELECT year FROM aarsmelding"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("no field 'year'. Its fields are: aar, navn"),
        "{body}"
    );
    let (_, body) = sql(&cookie, json!({"sql":"DELETE aarsmelding"})).await;
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("expected FROM (at character 8)"),
        "{body}"
    );

    // DELETE previews first and changes nothing.
    let delete = "DELETE FROM aarsmelding WHERE aar < 2020";
    let (status, preview) = sql(&cookie, json!({"sql":delete})).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["affected"], json!(2));
    assert_eq!(preview["done"], json!(false));
    assert_eq!(preview["changes"][0]["title"], json!("A"));
    let confirmation = preview["confirmation"].as_str().unwrap().to_string();
    let (_, count) = sql(&cookie, json!({"sql":"SELECT COUNT(*) FROM aarsmelding"})).await;
    assert_eq!(count["rows"], json!([[4]]));
    // Viewers can read but not change.
    let (viewer, _) = register_as(&app, "sql-viewer@example.com", "sqlviewer", Role::Viewer).await;
    let (status, _) = sql(&viewer, json!({"sql":"SELECT navn FROM aarsmelding"})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = sql(&viewer, json!({"sql":delete,"confirm":confirmation})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A confirmation only applies to the previewed records.
    create_document(&app, &cookie, &reports.id, json!({"aar":2017,"navn":"E"})).await;
    let (status, body) = sql(&cookie, json!({"sql":delete,"confirm":confirmation})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (_, preview) = sql(&cookie, json!({"sql":delete})).await;
    assert_eq!(preview["affected"], json!(3));
    let (status, done) = sql(
        &cookie,
        json!({"sql":delete,"confirm":preview["confirmation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["done"], json!(true));
    assert_eq!(done["affected"], json!(3));
    // Deleted records are in the Trash.
    let response = app
        .call(
            "GET",
            &format!("/api/v1/collections/{}/deleted", reports.id),
            Some(&cookie),
            None,
            None,
        )
        .await;
    let (_, trash) = json_of::<Value>(response).await;
    assert_eq!(trash["items"].as_array().unwrap().len(), 3, "{trash}");

    // UPDATE validates every record and records history.
    let (_, preview) = sql(
        &cookie,
        json!({"sql":"UPDATE aarsmelding SET navn = NULL WHERE kommune = 'Oslo'"}),
    )
    .await;
    assert_eq!(preview["confirmation"], Value::Null);
    assert!(
        preview["problems"][0].as_str().unwrap().contains("navn"),
        "{preview}"
    );
    let update = "UPDATE aarsmelding SET fte = fte * 2, kommune = UPPER(kommune) WHERE aar >= 2023 AND kommune LIKE 'b%'";
    let (_, preview) = sql(&cookie, json!({"sql":update})).await;
    assert_eq!(preview["affected"], json!(1));
    assert_eq!(
        preview["changes"][0]["before"],
        json!({"fte":4.0,"kommune":"Bergen"})
    );
    assert_eq!(
        preview["changes"][0]["after"],
        json!({"fte":8,"kommune":"BERGEN"})
    );
    let (status, _) = sql(
        &cookie,
        json!({"sql":update,"confirm":preview["confirmation"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, rows) = sql(
        &cookie,
        json!({"sql":"SELECT navn, fte, double, kommune FROM aarsmelding ORDER BY aar"}),
    )
    .await;
    assert_eq!(
        rows["rows"],
        json!([["C", 8, 16, "BERGEN"], ["D", 5.5, 11, "Oslo"]])
    );
    // Running it again changes nothing, so there is nothing to confirm.
    let (_, preview) = sql(
        &cookie,
        json!({"sql":"UPDATE aarsmelding SET kommune = 'BERGEN' WHERE navn = 'C'"}),
    )
    .await;
    assert_eq!(preview["affected"], json!(0));
    assert_eq!(preview["confirmation"], Value::Null);
    let (status, body) = sql(&cookie, json!({"sql":"UPDATE aarsmelding SET double = 1"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"].as_str().unwrap().contains("calculated"),
        "{body}"
    );
}

#[tokio::test]
async fn admins_make_download_and_delete_backups() {
    let app = TestApp::new().await;
    let (admin, _) = register(&app, "ada@example.com", "ada").await;
    let (editor, _) = register_as(&app, "grace@example.com", "grace", Role::Editor).await;
    let (_, key) = json_of::<CreatedApiKey>(
        app.call("POST", "/api/v1/api-keys", Some(&admin), None, Some(json!({"name":"admin key"})))
            .await,
    )
    .await;
    // A backup holds every account, so only admin sessions reach it.
    for (cookie, bearer) in [(Some(editor.as_str()), None), (None, Some(key.secret.as_str()))] {
        for method in ["GET", "POST"] {
            assert_eq!(
                app.call(method, "/api/v1/backups", cookie, bearer, None).await.status(),
                StatusCode::FORBIDDEN
            );
        }
    }

    let (status, made) = json_of::<vulse_model::BackupView>(
        app.call("POST", "/api/v1/backups", Some(&admin), None, None).await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(made.name.starts_with("vulse-backup-") && made.name.ends_with(".zip"));
    assert!(made.created_at.ends_with('Z'));
    let (_, overview) = json_of::<vulse_model::BackupOverview>(
        app.call("GET", "/api/v1/backups", Some(&admin), None, None).await,
    )
    .await;
    assert_eq!(overview.backups, vec![made.clone()]);
    assert_eq!(overview.every_hours, None);

    let download = app
        .call("GET", &format!("/api/v1/backups/{}", made.name), Some(&admin), None, None)
        .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers()["content-type"], "application/zip");
    assert!(download.headers()["content-disposition"].to_str().unwrap().contains(&made.name));
    let bytes = download.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.len() as u64, made.bytes);
    let zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(zip.file_names().any(|name| name == "vulse.db"));

    for name in ["vulse.db", "..%2Fvulse.db", "vulse-backup-..%2F..%2Fvulse.db.zip"] {
        assert_eq!(
            app.call("GET", &format!("/api/v1/backups/{name}"), Some(&admin), None, None)
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{name}"
        );
    }
    let path = format!("/api/v1/backups/{}", made.name);
    assert_eq!(app.call("DELETE", &path, Some(&editor), None, None).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(app.call("DELETE", &path, Some(&admin), None, None).await.status(), StatusCode::NO_CONTENT);
    assert_eq!(app.call("DELETE", &path, Some(&admin), None, None).await.status(), StatusCode::NOT_FOUND);
}
