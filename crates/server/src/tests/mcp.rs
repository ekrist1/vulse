//! The MCP endpoint: bearer-only access and the read tools.
use super::*;

const MCP_HEADERS: [(&str, &str); 3] = [
    ("host", "localhost"),
    ("accept", "application/json, text/event-stream"),
    ("mcp-protocol-version", "2025-06-18"),
];

pub(super) async fn mcp(
    app: &TestApp,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let response = app
        .call_with("POST", "/mcp", None, bearer, Some(body), &MCP_HEADERS)
        .await;
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// Calls a tool and returns its decoded JSON text and whether it is an error.
pub(super) async fn tool(
    app: &TestApp,
    bearer: &str,
    name: &str,
    arguments: Value,
) -> (Value, bool) {
    let (status, reply) = mcp(
        app,
        Some(bearer),
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let result = &reply["result"];
    assert!(result.is_object(), "{reply}");
    let text = result["content"][0]["text"].as_str().unwrap();
    let error = result["isError"].as_bool().unwrap_or(false);
    let value = if error {
        Value::String(text.to_string())
    } else {
        serde_json::from_str(text).unwrap()
    };
    (value, error)
}

async fn api_key(app: &TestApp, cookie: &str, body: Value) -> String {
    let response = app
        .call("POST", "/api/v1/api-keys", Some(cookie), None, Some(body))
        .await;
    let (status, key) = json_of::<CreatedApiKey>(response).await;
    assert_eq!(status, StatusCode::CREATED);
    key.secret
}

#[tokio::test]
async fn mcp_requires_a_bearer_credential() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});

    for (cookie, bearer) in [
        (None, None),
        (Some(cookie.as_str()), None),
        (None, Some("vk_nope")),
    ] {
        let response = app
            .call_with(
                "POST",
                "/mcp",
                cookie,
                bearer,
                Some(list.clone()),
                &MCP_HEADERS,
            )
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()["www-authenticate"],
            "Bearer resource_metadata=\"http://localhost/.well-known/oauth-protected-resource/mcp\""
        );
    }

    let key = api_key(&app, &cookie, json!({"name": "claude"})).await;
    let mut headers = MCP_HEADERS.to_vec();
    headers.push(("origin", "https://evil.example"));
    let response = app
        .call_with("POST", "/mcp", None, Some(&key), Some(list), &headers)
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn mcp_lists_tools_with_hints() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let key = api_key(&app, &cookie, json!({"name": "claude"})).await;

    let (status, reply) = mcp(&app, Some(&key), "tools/list", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    let tools = reply["result"]["tools"].as_array().unwrap();
    let hints = |name: &str| {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        let annotations = &tool["annotations"];
        (
            annotations["readOnlyHint"].as_bool().unwrap(),
            annotations["destructiveHint"].as_bool().unwrap_or(false),
        )
    };
    assert_eq!(tools.len(), 9);
    for name in [
        "list_collections",
        "describe_collection",
        "query_records",
        "get_record",
        "run_sql",
    ] {
        assert_eq!(hints(name), (true, false), "{name}");
    }
    for name in ["create_records", "update_records"] {
        assert_eq!(hints(name), (false, false), "{name}");
    }
    for name in ["delete_records", "undo_action"] {
        assert_eq!(hints(name), (false, true), "{name}");
    }
}

#[tokio::test]
async fn mcp_reads_collections_and_records() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let (_, _) = register_as(&app, "vi@example.com", "vi", Role::Viewer).await;
    let orders = create_collection(
        &app,
        &cookie,
        "orders",
        "private",
        json!({"fields":[
            {"name":"customer","type":"string"},
            {"name":"status","type":"string","options":["open","done"]},
            {"name":"price","type":"number"},
            {"name":"quantity","type":"number"},
            {"name":"total","type":"calculated","formula":"=[price]*[quantity]"}
        ]}),
    )
    .await;
    for (customer, status, price) in [("Ann", "open", 10), ("Bob", "done", 20), ("Cy", "open", 30)]
    {
        create_document(
            &app,
            &cookie,
            &orders.id,
            json!({"customer": customer, "status": status, "price": price, "quantity": 2}),
        )
        .await;
    }
    let key = api_key(&app, &cookie, json!({"name": "claude", "role": "viewer"})).await;

    let (listed, error) = tool(&app, &key, "list_collections", json!({})).await;
    assert!(!error);
    assert_eq!(listed["collections"][0]["name"], "orders");
    assert_eq!(listed["collections"][0]["fields"][4], "total");
    assert_eq!(listed["next_offset"], Value::Null);

    let (described, error) = tool(
        &app,
        &key,
        "describe_collection",
        json!({"collection": "Orders"}),
    )
    .await;
    assert!(!error, "{described}");
    assert_eq!(described["id"], orders.id);
    assert_eq!(described["fields"][1]["options"], json!(["open", "done"]));
    assert_eq!(described["fields"][4]["type"], "calculated");

    let (page, error) = tool(
        &app,
        &key,
        "query_records",
        json!({
            "collection": orders.id,
            "filters": [
                {"field": "status", "op": "eq", "value": "open"},
                {"field": "customer", "op": "eq", "value": "Bob", "or": true},
                {"field": "price", "op": "gte", "value": 20}
            ],
            "sort": "price",
            "descending": false,
            "limit": 1
        }),
    )
    .await;
    assert!(!error, "{page}");
    assert_eq!(page["total"], 2);
    assert_eq!(page["records"][0]["fields"]["customer"], "Bob");
    assert_eq!(page["records"][0]["fields"]["total"], 40.0);
    assert_eq!(page["next_offset"], 1);

    let id = page["records"][0]["id"].as_str().unwrap();
    let (record, error) = tool(
        &app,
        &key,
        "get_record",
        json!({"collection": "orders", "id": id}),
    )
    .await;
    assert!(!error, "{record}");
    assert_eq!(record["fields"]["customer"], "Bob");

    let (message, error) = tool(
        &app,
        &key,
        "query_records",
        json!({"collection": "orders", "filters": [{"field": "nope", "op": "eq", "value": 1}]}),
    )
    .await;
    assert!(error);
    assert_eq!(message, "unknown filter field");

    let (message, error) = tool(
        &app,
        &key,
        "get_record",
        json!({"collection": "orders", "id": "missing"}),
    )
    .await;
    assert!(error);
    assert_eq!(message, "no record missing in collection orders");

    let (message, error) = tool(
        &app,
        &key,
        "describe_collection",
        json!({"collection": "nope"}),
    )
    .await;
    assert!(error);
    assert!(message.as_str().unwrap().contains("list_collections"));
}

#[tokio::test]
async fn mcp_respects_collection_scoped_keys() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let fields = json!({"fields":[{"name":"title","type":"string"}]});
    let posts = create_collection(&app, &cookie, "posts", "private", fields.clone()).await;
    let secrets = create_collection(&app, &cookie, "secrets", "private", fields).await;
    let secret = create_document(&app, &cookie, &secrets.id, json!({"title": "hidden"})).await;
    let key = api_key(
        &app,
        &cookie,
        json!({"name": "posts", "collection_id": posts.id}),
    )
    .await;

    let (listed, _) = tool(&app, &key, "list_collections", json!({})).await;
    assert_eq!(listed["collections"].as_array().unwrap().len(), 1);
    assert_eq!(listed["collections"][0]["name"], "posts");

    for (name, arguments) in [
        ("describe_collection", json!({"collection": "secrets"})),
        ("query_records", json!({"collection": "secrets"})),
        (
            "get_record",
            json!({"collection": secrets.id, "id": secret.id}),
        ),
    ] {
        let (message, error) = tool(&app, &key, name, arguments).await;
        assert!(error, "{name}");
        assert_eq!(message, "this api key is limited to one collection");
    }
}

async fn mcp_tasks(app: &TestApp, cookie: &str) -> CollectionView {
    create_collection(
        app,
        cookie,
        "tasks",
        "private",
        json!({"fields":[
            {"name":"title","type":"string","required":true},
            {"name":"hours","type":"number"},
            {"name":"double","type":"calculated","formula":"=[hours]*2"}
        ]}),
    )
    .await
}

async fn count(app: &TestApp, cookie: &str, collection: &str) -> usize {
    let response = app
        .call(
            "GET",
            &format!("/api/v1/collections/{collection}/documents"),
            Some(cookie),
            None,
            None,
        )
        .await;
    json_of::<Page<DocumentView>>(response).await.1.items.len()
}

#[tokio::test]
async fn mcp_writes_need_the_editor_role() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let tasks = mcp_tasks(&app, &cookie).await;
    let doc = create_document(&app, &cookie, &tasks.id, json!({"title": "a"})).await;
    let key = api_key(&app, &cookie, json!({"name": "reader", "role": "viewer"})).await;

    for (name, arguments) in [
        (
            "create_records",
            json!({"collection": "tasks", "records": [{"title": "b"}]}),
        ),
        (
            "update_records",
            json!({"collection": "tasks", "updates": [{"id": doc.id, "fields": {"title": "c"}}]}),
        ),
        (
            "delete_records",
            json!({"collection": "tasks", "ids": [doc.id]}),
        ),
        (
            "undo_action",
            json!({"collection": "tasks", "undo_id": "x"}),
        ),
    ] {
        let (message, error) = tool(&app, &key, name, arguments).await;
        assert!(error, "{name}");
        assert_eq!(message, "this action requires the editor role", "{name}");
    }
    assert_eq!(count(&app, &cookie, &tasks.id).await, 1);
}

#[tokio::test]
async fn mcp_creates_updates_deletes_and_undoes_atomically() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let tasks = mcp_tasks(&app, &cookie).await;
    let key = api_key(&app, &cookie, json!({"name": "claude", "role": "editor"})).await;

    // One invalid record keeps the whole batch out.
    let (message, error) = tool(
        &app,
        &key,
        "create_records",
        json!({"collection": "tasks", "records": [{"title": "ok"}, {"hours": 1}]}),
    )
    .await;
    assert!(error, "{message}");
    assert_eq!(count(&app, &cookie, &tasks.id).await, 0);

    let (created, error) = tool(
        &app,
        &key,
        "create_records",
        json!({"collection": "tasks", "records": [
            {"title": "write docs", "hours": 2, "double": 99},
            {"title": "ship", "hours": 1}
        ]}),
    )
    .await;
    assert!(!error, "{created}");
    assert_eq!(created["created"], 2);
    assert_eq!(created["ignored_calculated_fields"], json!(["double"]));
    assert_eq!(created["records"][0]["fields"]["double"], 4.0);
    let first = created["records"][0]["id"].as_str().unwrap().to_string();
    let second = created["records"][1]["id"].as_str().unwrap().to_string();
    let first_version = created["records"][0]["updated_at"]
        .as_str()
        .unwrap()
        .to_string();

    // Each update has its own fields, and all of them are checked first.
    let (message, error) = tool(
        &app,
        &key,
        "update_records",
        json!({"collection": "tasks", "updates": [
            {"id": first, "fields": {"hours": 5}},
            {"id": second, "fields": {"double": 1}}
        ]}),
    )
    .await;
    assert!(error);
    assert_eq!(message, "calculated fields are read-only");
    let (message, error) = tool(
        &app,
        &key,
        "update_records",
        json!({"collection": "tasks", "updates": [{"id": "missing", "fields": {"hours": 5}}]}),
    )
    .await;
    assert!(error);
    assert_eq!(message, "a record id was not found in collection tasks");

    let (updated, error) = tool(
        &app,
        &key,
        "update_records",
        json!({"collection": "tasks", "updates": [
            {"id": first, "fields": {"hours": 5}, "expected_updated_at": first_version},
            {"id": second, "fields": {"title": "ship it"}}
        ]}),
    )
    .await;
    assert!(!error, "{updated}");
    assert_eq!(updated["updated"], 2);
    assert_eq!(updated["records"][0]["fields"]["double"], 10.0);
    assert_eq!(updated["records"][1]["fields"]["title"], "ship it");
    assert_eq!(updated["records"][1]["fields"]["hours"], 1);

    let (message, error) = tool(
        &app,
        &key,
        "update_records",
        json!({"collection": "tasks", "updates": [
            {"id": first, "fields": {"hours": 6}, "expected_updated_at": "2000-01-01T00:00:00Z"}
        ]}),
    )
    .await;
    assert!(error);
    assert!(
        message
            .as_str()
            .unwrap()
            .contains("changed since you loaded it")
    );

    // Changes made over MCP are marked in the record's history.
    let response = app
        .call(
            "GET",
            &format!(
                "/api/v1/collections/{}/documents/{first}/revisions",
                tasks.id
            ),
            Some(&cookie),
            None,
            None,
        )
        .await;
    let (_, revisions) = json_of::<Value>(response).await;
    let revisions = revisions["items"].as_array().unwrap();
    assert_eq!(revisions.len(), 2);
    assert!(
        revisions.iter().all(|r| r["source"] == "mcp"),
        "{revisions:?}"
    );

    let undo = updated["undo_id"].as_str().unwrap();
    let (undone, error) = tool(
        &app,
        &key,
        "undo_action",
        json!({"collection": "tasks", "undo_id": undo}),
    )
    .await;
    assert!(!error, "{undone}");
    assert_eq!(undone["undone"], 2);
    let (record, _) = tool(
        &app,
        &key,
        "get_record",
        json!({"collection": "tasks", "id": second}),
    )
    .await;
    assert_eq!(record["fields"]["title"], "ship");
    let (message, error) = tool(
        &app,
        &key,
        "undo_action",
        json!({"collection": "tasks", "undo_id": undo}),
    )
    .await;
    assert!(error);
    assert!(message.as_str().unwrap().contains("may already be undone"));

    let (deleted, error) = tool(
        &app,
        &key,
        "delete_records",
        json!({"collection": "tasks", "ids": [first, second]}),
    )
    .await;
    assert!(!error, "{deleted}");
    assert_eq!(deleted["deleted"], 2);
    assert_eq!(count(&app, &cookie, &tasks.id).await, 0);
    let response = app
        .call(
            "GET",
            &format!("/api/v1/collections/{}/deleted", tasks.id),
            Some(&cookie),
            None,
            None,
        )
        .await;
    let (_, trash) = json_of::<Value>(response).await;
    assert_eq!(trash["items"].as_array().unwrap().len(), 2);

    let (_, error) = tool(
        &app,
        &key,
        "undo_action",
        json!({"collection": "tasks", "undo_id": deleted["undo_id"]}),
    )
    .await;
    assert!(!error);
    assert_eq!(count(&app, &cookie, &tasks.id).await, 2);

    // Undoing a create removes the records it made.
    let (_, error) = tool(
        &app,
        &key,
        "undo_action",
        json!({"collection": "tasks", "undo_id": created["undo_id"]}),
    )
    .await;
    assert!(error, "the records changed after they were created");
    let (more, _) = tool(
        &app,
        &key,
        "create_records",
        json!({"collection": "tasks", "records": [{"title": "temp"}]}),
    )
    .await;
    let (_, error) = tool(
        &app,
        &key,
        "undo_action",
        json!({"collection": "tasks", "undo_id": more["undo_id"]}),
    )
    .await;
    assert!(!error);
    assert_eq!(count(&app, &cookie, &tasks.id).await, 2);
}

#[tokio::test]
async fn mcp_runs_read_only_sql() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let fields = json!({"fields":[
        {"name":"customer","type":"string"},
        {"name":"price","type":"number"}
    ]});
    let orders = create_collection(&app, &cookie, "orders", "private", fields.clone()).await;
    create_collection(&app, &cookie, "secrets", "private", fields).await;
    for (customer, price) in [("Ann", 10), ("Ann", 30), ("Bob", 5)] {
        create_document(
            &app,
            &cookie,
            &orders.id,
            json!({"customer": customer, "price": price}),
        )
        .await;
    }
    let key = api_key(
        &app,
        &cookie,
        json!({"name": "orders", "collection_id": orders.id}),
    )
    .await;

    let (result, error) = tool(
        &app,
        &key,
        "run_sql",
        json!({"sql": "SELECT customer, SUM(price) AS spent FROM orders GROUP BY customer ORDER BY spent DESC"}),
    )
    .await;
    assert!(!error, "{result}");
    assert_eq!(result["columns"], json!(["customer", "spent"]));
    assert_eq!(result["rows"][0], json!(["Ann", 40]));
    assert_eq!(result["total"], 2);
    assert_eq!(result["truncated"], false);

    for (sql, expected) in [
        (
            "UPDATE orders SET price = 0",
            "Only SELECT statements can run here.",
        ),
        ("DELETE FROM orders", "Only SELECT statements can run here."),
        (
            "SELECT * FROM secrets",
            "this api key is limited to one collection",
        ),
    ] {
        let (message, error) = tool(&app, &key, "run_sql", json!({"sql": sql})).await;
        assert!(error, "{sql}");
        assert!(message.as_str().unwrap().starts_with(expected), "{message}");
    }
    assert_eq!(count(&app, &cookie, &orders.id).await, 3);
}
