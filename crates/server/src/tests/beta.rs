//! Regression coverage for the first-beta review findings.
use super::*;

#[tokio::test]
async fn exhausted_writer_wait_returns_retryable_error_without_writing() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "busy@example.com", "busy").await;
    let collection = create_collection(
        &app,
        &cookie,
        "busy",
        "private",
        json!({"fields":[{"name":"name","type":"string"}]}),
    )
    .await;
    let path = format!("/api/v1/collections/{}/documents", collection.id);
    let writer = app.db.connect().await.unwrap();
    writer.execute("BEGIN IMMEDIATE", &[]).await.unwrap();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        app.call(
            "POST",
            &path,
            Some(&cookie),
            None,
            Some(json!({"body":{"name":"retry me"}})),
        ),
    )
    .await
    .expect("the writer wait is bounded");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    writer.execute("ROLLBACK", &[]).await.unwrap();
    assert!(
        crate::store::list_documents(&writer, &collection.id, 100, 0)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.call(
            "POST",
            &path,
            Some(&cookie),
            None,
            Some(json!({"body":{"name":"retry me"}}))
        )
        .await
        .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        crate::store::list_documents(&writer, &collection.id, 100, 0)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn summary_reference_titles_respect_public_and_key_access() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "summary-access@example.com", "summary-access").await;
    let private = create_collection(
        &app,
        &cookie,
        "private",
        "private",
        json!({"title_field":"name","fields":[{"name":"name","type":"string"}]}),
    )
    .await;
    let secret = create_document(
        &app,
        &cookie,
        &private.id,
        json!({"name":"Confidential title"}),
    )
    .await;
    let public = create_collection(
        &app,
        &cookie,
        "public",
        "public",
        json!({"title_field":"name","fields":[
            {"name":"name","type":"string"},
            {"name":"customer","type":"reference","collection":"private"},
            {"name":"self_ref","type":"reference","collection":"public"}
        ]}),
    )
    .await;
    let title = create_document(&app, &cookie, &public.id, json!({"name":"Public title"})).await;
    create_document(
        &app,
        &cookie,
        &public.id,
        json!({"customer":secret.id,"self_ref":title.id}),
    )
    .await;
    let summary = json!({"group_by":[{"field":"customer"},{"field":"self_ref"}],"measures":[{"name":"count","op":"count"}]});
    let (status, key) = json_of::<CreatedApiKey>(
        app.call(
            "POST",
            "/api/v1/api-keys",
            Some(&cookie),
            None,
            Some(json!({"name":"limited","collection_id":public.id,"role":"viewer"})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, view) = json_of::<Value>(
        app.call(
            "POST",
            &format!("/api/v1/collections/{}/views", public.id),
            Some(&cookie),
            None,
            Some(json!({"name":"Grouped","query":{},"columns":["customer"],"summary":summary})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    let saved_path = format!(
        "/api/v1/collections/{}/views/{}/summary",
        public.id,
        view["id"].as_str().unwrap()
    );
    for (method, path, session, bearer, body) in [
        (
            "POST",
            "/api/v1/public/public/summary".to_string(),
            None,
            None,
            Some(json!({"summary":summary})),
        ),
        (
            "POST",
            format!("/api/v1/collections/{}/summary", public.id),
            None,
            Some(key.secret.as_str()),
            Some(json!({"summary":summary})),
        ),
        ("GET", saved_path, None, Some(key.secret.as_str()), None),
    ] {
        let (status, result) =
            json_of::<Value>(app.call(method, &path, session, bearer, body).await).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        let row = result["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["group"][0] == secret.id)
            .unwrap();
        assert_eq!(row["labels"], json!([secret.id, "Public title"]));
        assert!(!result.to_string().contains("Confidential title"));
    }
    let (status, result) = json_of::<Value>(
        app.call(
            "POST",
            &format!("/api/v1/collections/{}/summary", public.id),
            Some(&cookie),
            None,
            Some(json!({"summary":summary})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(result.to_string().contains("Confidential title"));
    // Public references still resolve titles when the target is public.
    assert_eq!(
        app.call(
            "PATCH",
            &format!("/api/v1/collections/{}", private.id),
            Some(&cookie),
            None,
            Some(json!({"visibility":"public"}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let (_, result) = json_of::<Value>(
        app.call(
            "POST",
            "/api/v1/public/public/summary",
            None,
            None,
            Some(json!({"summary":summary})),
        )
        .await,
    )
    .await;
    assert!(result.to_string().contains("Confidential title"));
}

#[tokio::test]
async fn summary_counts_match_not_empty_filters() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "count@example.com", "counts").await;
    let collection = create_collection(
        &app,
        &cookie,
        "counts",
        "private",
        json!({"fields":[
            {"name":"name","type":"string"},{"name":"tags","type":"array","items":"string"},
            {"name":"number","type":"number"},{"name":"flag","type":"boolean"}
        ]}),
    )
    .await;
    for body in [
        json!({"name":"Original","tags":["a"],"number":0,"flag":false}),
        json!({"name":"","tags":[]}),
        json!({"name":null,"tags":null}),
        json!({}),
    ] {
        create_document(&app, &cookie, &collection.id, body).await;
    }
    for field in ["name", "tags", "number", "flag"] {
        let (status, result) = json_of::<Value>(app.call("POST", &format!("/api/v1/collections/{}/summary", collection.id), Some(&cookie), None, Some(json!({"summary":{"measures":[{"name":"filled","op":"count","field":field},{"name":"all","op":"count"}]}}))).await).await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert_eq!(result["totals"]["values"]["filled"], 1, "{field}: {result}");
        assert_eq!(result["totals"]["values"]["all"], 4);
        let (_, query) = json_of::<Value>(
            app.call(
                "POST",
                &format!("/api/v1/collections/{}/query", collection.id),
                Some(&cookie),
                None,
                Some(json!({"filters":[{"field":field,"op":"not_empty"}]})),
            )
            .await,
        )
        .await;
        assert_eq!(query["total"], result["totals"]["values"]["filled"]);
    }
}

#[tokio::test]
async fn concurrent_writes_wait_and_read_snapshots_do_not_reserve_writer() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "concurrent@example.com", "concurrent").await;
    let collection = create_collection(
        &app,
        &cookie,
        "concurrent",
        "private",
        json!({"fields":[{"name":"name","type":"string"}]}),
    )
    .await;
    let path = format!("/api/v1/collections/{}/documents", collection.id);
    let requests = (0..12).map(|i| {
        app.call(
            "POST",
            &path,
            Some(&cookie),
            None,
            Some(json!({"body":{"name":format!("Record {i}")}})),
        )
    });
    let results = futures_util::future::join_all(requests).await;
    for response in results {
        let (status, body) = json_of::<Value>(response).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let conn = app.db.connect().await.unwrap();
    conn.with_app_read_tx(async |conn| {
        assert_eq!(
            crate::store::list_documents(conn, &collection.id, 100, 0)
                .await?
                .len(),
            12
        );
        let response = app
            .call(
                "POST",
                &path,
                Some(&cookie),
                None,
                Some(json!({"body":{"name":"During read snapshot"}})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            crate::store::list_documents(conn, &collection.id, 100, 0)
                .await?
                .len(),
            12
        );
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        crate::store::list_documents(&conn, &collection.id, 100, 0)
            .await
            .unwrap()
            .len(),
        13
    );
}

#[tokio::test]
async fn bulk_lookup_cache_sees_all_writes_and_never_exposes_uncommitted_values() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "cache@example.com", "cache").await;
    let collection = create_collection(&app, &cookie, "amounts", "private", json!({"fields":[{"name":"amount","type":"number"},{"name":"total","type":"calculated","formula":"=SUM([amounts.amount])"}]})).await;
    let a = create_document(&app, &cookie, &collection.id, json!({"amount":1})).await;
    let b = create_document(&app, &cookie, &collection.id, json!({"amount":2})).await;
    let path = format!("/api/v1/collections/{}/documents/{}", collection.id, a.id);
    let read = async || {
        let (status, doc) =
            json_of::<DocumentView>(app.call("GET", &path, Some(&cookie), None, None).await).await;
        assert_eq!(status, StatusCode::OK);
        doc.body["total"].as_f64().unwrap()
    };
    assert_eq!(read().await, 3.0);
    let (status, result) = json_of::<vulse_model::MutationResult>(
        app.call(
            "POST",
            &format!("/api/v1/collections/{}/bulk", collection.id),
            Some(&cookie),
            None,
            Some(json!({"action":"update","ids":[a.id,b.id],"patch":{"amount":10}})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for doc in &result.documents {
        assert_eq!(doc.body["total"].as_f64(), Some(20.0));
    }
    assert_eq!(read().await, 20.0);
    let conn = app.db.connect().await.unwrap();
    let failed: Result<(), crate::error::AppError> = conn
        .with_app_tx(async |conn| {
            // Deliberately reuse a timestamp, as bulk writes do.
            crate::store::update_document(
                conn,
                &collection.id,
                &a.id,
                &json!({"amount":99}),
                &result.documents[0].updated_at,
            )
            .await?;
            let lookups = crate::lookups::load(conn, &collection.blueprint).await?;
            let body =
                vulse_model::calculate_document_with(&collection.blueprint, &json!({}), &lookups).0;
            assert_eq!(body["total"].as_f64(), Some(109.0));
            assert_eq!(read().await, 20.0);
            Err(crate::error::AppError::bad("force rollback"))
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(read().await, 20.0);
    let response = app
        .call(
            "POST",
            &format!(
                "/api/v1/collections/{}/actions/{}/undo",
                collection.id, result.undo_id
            ),
            Some(&cookie),
            None,
            Some(json!({})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(read().await, 3.0);
}

#[tokio::test]
async fn indirect_lookup_chains_are_rejected_on_create_update_and_legacy_reads() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "chain@example.com", "chain").await;
    let raw = create_collection(
        &app,
        &cookie,
        "raw",
        "private",
        json!({"fields":[{"name":"amount","type":"number"}]}),
    )
    .await;
    create_document(&app, &cookie, &raw.id, json!({"amount":5})).await;
    let fields = |formula: &str| json!({"fields":[{"name":"direct","type":"calculated","formula":formula},{"name":"indirect","type":"calculated","formula":"=[direct]*2"}]});
    let middle = create_collection(
        &app,
        &cookie,
        "middle",
        "private",
        fields("=SUM([raw.amount])"),
    )
    .await;
    create_document(&app, &cookie, &middle.id, json!({})).await;
    let consumer = json!({"name":"consumer","visibility":"private","blueprint":{"fields":[{"name":"result","type":"calculated","formula":"=SUM([middle.indirect])"}]}});
    let (status, error) = json_of::<Value>(
        app.call(
            "POST",
            "/api/v1/collections",
            Some(&cookie),
            None,
            Some(consumer.clone()),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(
        app.call(
            "PATCH",
            &format!("/api/v1/collections/{}", middle.id),
            Some(&cookie),
            None,
            Some(json!({"blueprint":fields("=5")}))
        )
        .await
        .status(),
        StatusCode::OK
    );
    let (status, consumer) = json_of::<CollectionView>(
        app.call(
            "POST",
            "/api/v1/collections",
            Some(&cookie),
            None,
            Some(consumer),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let doc = create_document(&app, &cookie, &consumer.id, json!({})).await;
    assert_eq!(doc.body["result"].as_f64(), Some(10.0));
    assert_eq!(
        app.call(
            "PATCH",
            &format!("/api/v1/collections/{}", middle.id),
            Some(&cookie),
            None,
            Some(json!({"blueprint":fields("=SUM([raw.amount])")}))
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    // Simulate an indirect chain persisted by an older server.
    let conn = app.db.connect().await.unwrap();
    conn.execute(
        "UPDATE collections SET blueprint=?1 WHERE id=?2",
        &[
            crate::db::SqlValue::Text(fields("=SUM([raw.amount])").to_string()),
            crate::db::SqlValue::Text(middle.id.clone()),
        ],
    )
    .await
    .unwrap();
    let (status, doc) = json_of::<DocumentView>(
        app.call(
            "GET",
            &format!("/api/v1/collections/{}/documents/{}", consumer.id, doc.id),
            Some(&cookie),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(doc.body["result"].is_null());
    assert!(doc.calculation_errors.contains_key("result"));
}
