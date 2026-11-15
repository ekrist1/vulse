use super::*;
use crate::db::{SqlValue, int_at};
use vulse_model::SummaryResult;

async fn summary(app: &TestApp, cookie: &str, id: &str, spec: Value) -> SummaryResult {
    let (status, value) = json_of::<Value>(
        app.call(
            "POST",
            &format!("/api/v1/collections/{id}/summary"),
            Some(cookie),
            None,
            Some(json!({"summary":spec})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn source_errors_propagate_and_recover_after_writes_import_delete_and_undo() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "integrity@example.com", "integrity").await;
    let source = create_collection(
        &app,
        &cookie,
        "source",
        "public",
        json!({"fields":[
            {"name":"group","type":"string"}, {"name":"divisor","type":"number"},
            {"name":"ratio","type":"calculated","formula":"=10/[divisor]"},
            {"name":"twice","type":"calculated","formula":"=[ratio]*2"}
        ]}),
    )
    .await;
    let reader = create_collection(
        &app,
        &cookie,
        "reader",
        "private",
        json!({"fields":[
            {"name":"total","type":"calculated","formula":"=SUM([source.ratio])"},
            {"name":"double","type":"calculated","formula":"=[total]*2"}
        ]}),
    )
    .await;
    create_document(&app, &cookie, &reader.id, json!({})).await;
    create_document(
        &app,
        &cookie,
        &source.id,
        json!({"group":"good","divisor":2}),
    )
    .await;
    let bad = create_document(
        &app,
        &cookie,
        &source.id,
        json!({"group":"bad","divisor":0}),
    )
    .await;
    let spec = json!({"group_by":[{"field":"group"}],"measures":[
        {"name":"sum","op":"sum","field":"ratio"},
        {"name":"derived","op":"formula","formula":"=[sum]*2"},
        {"name":"direct","op":"formula","formula":"=SUM([record.twice])"},
        {"name":"count","op":"count"}
    ]});
    let result = summary(&app, &cookie, &source.id, spec.clone()).await;
    for name in ["sum", "derived", "direct"] {
        assert!(result.totals.values[name].is_null());
        assert!(result.totals.errors[name].contains("source records"));
    }
    assert_eq!(result.totals.values["count"], 2);
    let good = result.rows.iter().find(|r| r.group[0] == "good").unwrap();
    assert_eq!(good.values["sum"], 5);
    assert!(good.errors.is_empty());
    let reader_spec = json!({"measures":[{"name":"s","op":"sum","field":"double"}]});
    assert!(
        summary(&app, &cookie, &reader.id, reader_spec.clone())
            .await
            .totals
            .errors
            .contains_key("s")
    );
    // Errors used to disappear when loading a calculated field as a lookup range.
    let (_, public) = json_of::<SummaryResult>(
        app.call(
            "POST",
            "/api/v1/public/source/summary",
            None,
            None,
            Some(json!({"summary":spec})),
        )
        .await,
    )
    .await;
    assert!(public.totals.errors.contains_key("derived"));
    let filtered = app.call("POST", &format!("/api/v1/collections/{}/summary", source.id), Some(&cookie), None,
        Some(json!({"query":{"filters":[{"field":"ratio","op":"gt","value":0}]}, "summary":spec}))).await;
    assert_eq!(filtered.status(), StatusCode::BAD_REQUEST);
    // A correction clears both local and cross-collection errors in the same commit.
    let response = app
        .call(
            "PUT",
            &format!("/api/v1/collections/{}/documents/{}", source.id, bad.id),
            Some(&cookie),
            None,
            Some(json!({"body":{"group":"bad","divisor":5}})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let corrected = summary(&app, &cookie, &source.id, spec.clone()).await;
    assert_eq!(corrected.totals.values["sum"], 7);
    assert!(corrected.totals.errors.is_empty());
    assert_eq!(
        summary(&app, &cookie, &reader.id, reader_spec.clone())
            .await
            .totals
            .values["s"],
        14
    );
    let (status, imported) = json_of::<Value>(
        import_csv(
            &app,
            Some(&cookie),
            &source.id,
            "group,divisor\nimported,10\n",
            true,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    assert_eq!(
        summary(&app, &cookie, &reader.id, reader_spec.clone())
            .await
            .totals
            .values["s"],
        16
    );
    let (status, deleted) = json_of::<vulse_model::MutationResult>(
        app.call(
            "POST",
            &format!("/api/v1/collections/{}/bulk", source.id),
            Some(&cookie),
            None,
            Some(json!({"action":"delete","ids":[bad.id]})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        summary(&app, &cookie, &reader.id, reader_spec.clone())
            .await
            .totals
            .values["s"],
        12
    );
    let undo = app
        .call(
            "POST",
            &format!(
                "/api/v1/collections/{}/actions/{}/undo",
                source.id, deleted.undo_id
            ),
            Some(&cookie),
            None,
            None,
        )
        .await;
    assert_eq!(undo.status(), StatusCode::OK);
    assert_eq!(
        summary(&app, &cookie, &reader.id, reader_spec)
            .await
            .totals
            .values["s"],
        16
    );
}

#[tokio::test]
async fn concurrent_commit_between_totals_and_groups_keeps_one_snapshot() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "snapshot@example.com", "snapshot").await;
    let collection = create_collection(
        &app,
        &cookie,
        "snapshot",
        "private",
        json!({"fields":[
            {"name":"group","type":"string"},{"name":"amount","type":"number"}
        ]}),
    )
    .await;
    let doc = create_document(
        &app,
        &cookie,
        &collection.id,
        json!({"group":"before","amount":10}),
    )
    .await;
    let spec = json!({"group_by":[{"field":"group"}],"measures":[{"name":"s","op":"formula","formula":"=SUM([record.amount])"}]});
    let (reached, resume) = crate::summary::pause_after_totals(&collection.id);
    let read = summary(&app, &cookie, &collection.id, spec.clone());
    let write = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), reached)
            .await
            .unwrap()
            .unwrap();
        let response = app
            .call(
                "PUT",
                &format!("/api/v1/collections/{}/documents/{}", collection.id, doc.id),
                Some(&cookie),
                None,
                Some(json!({"body":{"group":"after","amount":20}})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        resume.send(()).unwrap();
    };
    let (result, _) = tokio::join!(read, write);
    assert_eq!(result.totals.values["s"], 10);
    assert_eq!(result.rows[0].values["s"], 10);
    assert_eq!(result.rows[0].group[0], "before");
    let next = summary(&app, &cookie, &collection.id, spec).await;
    assert_eq!(next.totals.values["s"], 20);
    assert_eq!(next.rows[0].group[0], "after");
}

#[tokio::test]
async fn shared_versions_notify_without_local_publish_and_rollback_is_invisible() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "shared@example.com", "shared").await;
    let collection = create_collection(
        &app,
        &cookie,
        "shared",
        "private",
        json!({"fields":[{"name":"amount","type":"number"}]}),
    )
    .await;
    let doc = create_document(&app, &cookie, &collection.id, json!({"amount":1})).await;
    let response = app
        .call(
            "GET",
            &format!("/api/v1/collections/{}/events", collection.id),
            Some(&cookie),
            None,
            None,
        )
        .await;
    let mut stream = response.into_body();
    let conn = app.db.connect().await.unwrap();
    let params = [SqlValue::Text(collection.id.clone())];
    let blueprint: vulse_model::Blueprint = serde_json::from_value(json!({"fields":[
        {"name":"total","type":"calculated","formula":"=SUM([shared.amount])"}
    ]}))
    .unwrap();
    let cached = crate::lookups::load(&conn, &blueprint).await.unwrap();
    assert_eq!(
        vulse_model::calculate_document_with(&blueprint, &json!({}), &cached).0["total"],
        1.0
    );
    let before = int_at(
        &conn
            .query(
                "SELECT version FROM collection_versions WHERE collection_id=?1",
                &params,
            )
            .await
            .unwrap()[0],
        0,
    )
    .unwrap();
    conn.execute("BEGIN IMMEDIATE", &[]).await.unwrap();
    conn.execute(
        "UPDATE documents SET body='{\"amount\":2}' WHERE id=?1",
        &[SqlValue::Text(doc.id.clone())],
    )
    .await
    .unwrap();
    conn.execute("ROLLBACK", &[]).await.unwrap();
    assert_eq!(
        before,
        int_at(
            &conn
                .query(
                    "SELECT version FROM collection_versions WHERE collection_id=?1",
                    &params
                )
                .await
                .unwrap()[0],
            0
        )
        .unwrap()
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(1200), stream.frame())
            .await
            .is_err()
    );
    // No Conn::changed / local broadcast: exactly the signal available from another process.
    conn.execute(
        "UPDATE documents SET body='{\"amount\":3}' WHERE id=?1",
        &[SqlValue::Text(doc.id)],
    )
    .await
    .unwrap();
    let refreshed = crate::lookups::load(&conn, &blueprint).await.unwrap();
    assert_eq!(
        vulse_model::calculate_document_with(&blueprint, &json!({}), &refreshed).0["total"],
        3.0
    );
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), stream.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
    assert!(text.contains("event: change"), "{text}");
}

#[tokio::test]
async fn recalculating_50001_dependent_records_is_bounded_and_atomic() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "large-deps@example.com", "largedeps").await;
    let source = create_collection(
        &app,
        &cookie,
        "large-source",
        "private",
        json!({"fields":[{"name":"amount","type":"number"}]}),
    )
    .await;
    let reader = create_collection(
        &app,
        &cookie,
        "large-reader",
        "private",
        json!({"fields":[
            {"name":"total","type":"calculated","formula":"=SUM([large-source.amount])"}
        ]}),
    )
    .await;
    let conn = app.db.connect().await.unwrap();
    conn.with_app_tx(async |conn| {
        for (id, prefix, body) in [(&source.id, "s", "{\"amount\":1}"), (&reader.id, "r", "{}")] {
            conn.execute(
                "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<50001)
                INSERT INTO documents(id,collection_id,body,created_at,updated_at)
                SELECT ?1||n,?2,?3,'2026-01-01','2026-01-01' FROM seq",
                &[
                    SqlValue::Text(prefix.into()),
                    SqlValue::Text(id.clone()),
                    SqlValue::Text(body.into()),
                ],
            )
            .await?;
            conn.changed(id);
        }
        crate::rows::refresh(conn, &source.id, &source.blueprint, &Default::default()).await?;
        Ok(())
    })
    .await
    .unwrap();
    let spec = json!({"measures":[{"name":"s","op":"sum","field":"total"}]});
    assert_eq!(
        summary(&app, &cookie, &reader.id, spec.clone())
            .await
            .totals
            .values["s"],
        2500100001_i64
    );
    let started = std::time::Instant::now();
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        conn.with_app_tx(async |conn| {
            crate::store::update_document(
                conn,
                &source.id,
                "s1",
                &json!({"amount":2}),
                "2026-01-02",
            )
            .await?;
            Ok(())
        }),
    )
    .await
    .expect("whole-column aggregate must not scan 50k inputs for each of 50k outputs")
    .unwrap();
    eprintln!(
        "Updating a source with 50,001 source and 50,001 dependent records: {:?}",
        started.elapsed()
    );
    let result = summary(&app, &cookie, &reader.id, spec).await;
    assert_eq!(result.totals.count, 50001);
    assert_eq!(result.totals.values["s"], 2500150002_i64);
    assert!(result.totals.errors.is_empty());
}

#[tokio::test]
async fn upgrading_record_tables_backfills_calculation_errors() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "upgrade-errors@example.com", "upgradeerrors").await;
    let collection = create_collection(
        &app,
        &cookie,
        "upgrade-errors",
        "private",
        json!({"fields":[
            {"name":"ratio","type":"calculated","formula":"=1/0"}
        ]}),
    )
    .await;
    create_document(&app, &cookie, &collection.id, json!({})).await;
    let conn = app.db.connect().await.unwrap();
    let table = crate::rows::table(&collection.id);
    conn.execute(&format!("DROP TABLE {table}"), &[])
        .await
        .unwrap();
    conn.execute(&format!("CREATE TABLE {table}(id TEXT PRIMARY KEY)"), &[])
        .await
        .unwrap();
    conn.execute(
        "UPDATE workspace_settings SET value='2' WHERE key='record_tables_version'",
        &[],
    )
    .await
    .unwrap();
    crate::records::rebuild_outdated_tables(&conn)
        .await
        .unwrap();
    let result = summary(
        &app,
        &cookie,
        &collection.id,
        json!({"measures":[{"name":"s","op":"sum","field":"ratio"}]}),
    )
    .await;
    assert!(result.totals.values["s"].is_null());
    assert!(result.totals.errors.contains_key("s"));
}

#[tokio::test]
async fn saved_and_public_summaries_share_consistent_snapshots() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "snapshot-routes@example.com", "snapshotroutes").await;
    let collection = create_collection(
        &app,
        &cookie,
        "snapshot-routes",
        "public",
        json!({"fields":[
            {"name":"group","type":"string"},{"name":"amount","type":"number"}
        ]}),
    )
    .await;
    let doc = create_document(
        &app,
        &cookie,
        &collection.id,
        json!({"group":"a","amount":1}),
    )
    .await;
    let spec = json!({"group_by":[{"field":"group"}],"measures":[{"name":"s","op":"sum","field":"amount"}]});
    let (status, view) = json_of::<vulse_model::SavedView>(
        app.call(
            "POST",
            &format!("/api/v1/collections/{}/views", collection.id),
            Some(&cookie),
            None,
            Some(json!({"name":"Snapshot","query":{},"columns":["amount"],"summary":spec})),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    for (index, (method, url, body, auth)) in [
        (
            "POST",
            "/api/v1/public/snapshot-routes/summary".into(),
            Some(json!({"summary":spec})),
            None,
        ),
        (
            "GET",
            format!(
                "/api/v1/collections/{}/views/{}/summary",
                collection.id, view.id
            ),
            None,
            Some(cookie.as_str()),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (reached, resume) = crate::summary::pause_after_totals(&collection.id);
        let read = async {
            json_of::<SummaryResult>(app.call(method, &url, auth, None, body).await).await
        };
        let write = async {
            tokio::time::timeout(std::time::Duration::from_secs(10), reached)
                .await
                .unwrap()
                .unwrap();
            let response = app
                .call(
                    "PUT",
                    &format!("/api/v1/collections/{}/documents/{}", collection.id, doc.id),
                    Some(&cookie),
                    None,
                    Some(json!({"body":{"group":"a","amount":index+2}})),
                )
                .await;
            assert_eq!(response.status(), StatusCode::OK);
            resume.send(()).unwrap();
        };
        let ((status, result), _) = tokio::join!(read, write);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result.totals.values["s"], index + 1);
        assert_eq!(result.rows[0].values["s"], index + 1);
    }
}
