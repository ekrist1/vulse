//! Write performance: a large CSV import, adding calculated fields that
//! read a large collection, and single-record edits that refresh the
//! collections reading the edited one. Ignored by default; run with
//!
//! ```bash
//! VULSE_BENCH_ROWS=50000 cargo test --release -p vulse-server bench_import_and_formulas -- --ignored --nocapture
//! ```
//!
//! `VULSE_BENCH_EDITS` sets the number of edits per case (default 10). The
//! database's write-ahead log can grow to gigabytes over a run, so point
//! `TMPDIR` at a disk when `/tmp` is a small in-memory file system.

use std::time::Instant;

use axum::http::StatusCode;
use serde_json::{Value, json};
use vulse_model::{CollectionView, ImportResult};

use super::{TestApp, create_collection, import_csv, json_of, register};

const CUSTOMERS: usize = 1000;

fn orders_csv(rows: usize) -> String {
    let mut csv =
        String::from("order_no,customer,product,category,amount,qty,ordered,status,city,paid\n");
    for i in 0..rows {
        csv.push_str(&format!(
            "ORD-{i},C{},product {},category {},{}.{:02},{},2026-{:02}-{:02},{},city {},{}\n",
            i % CUSTOMERS,
            i % 300,
            i % 20,
            (i * 37) % 10_000,
            i % 100,
            1 + i % 9,
            1 + i % 12,
            1 + i % 28,
            ["open", "shipped", "closed"][i % 3],
            i % 200,
            i % 2 == 0
        ));
    }
    csv
}

fn customers_csv() -> String {
    let mut csv = String::from("code,name\n");
    for i in 0..CUSTOMERS {
        csv.push_str(&format!("C{i},Customer {i}\n"));
    }
    csv
}

async fn put_blueprint(app: &TestApp, cookie: &str, c: &CollectionView, blueprint: Value) -> f64 {
    let started = Instant::now();
    let response = app
        .call(
            "PATCH",
            &format!("/api/v1/collections/{}", c.id),
            Some(cookie),
            None,
            Some(json!({"blueprint": blueprint})),
        )
        .await;
    let (status, body) = json_of::<Value>(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    started.elapsed().as_secs_f64()
}

/// Edits `count` of the collection's first records one at a time through the
/// API and returns the mean time per edit in milliseconds.
async fn edit_records(
    app: &TestApp,
    cookie: &str,
    collection_id: &str,
    count: usize,
    change: impl Fn(&mut Value, usize),
) -> f64 {
    let (_, page) = json_of::<Value>(
        app.call(
            "POST",
            &format!("/api/v1/collections/{collection_id}/query"),
            Some(cookie),
            None,
            Some(json!({"limit": count})),
        )
        .await,
    )
    .await;
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), count);
    let mut total = 0.0;
    for (i, item) in items.iter().enumerate() {
        let mut body = item["body"].clone();
        change(&mut body, i);
        let started = Instant::now();
        let response = app
            .call(
                "PUT",
                &format!(
                    "/api/v1/collections/{collection_id}/documents/{}",
                    item["id"].as_str().unwrap()
                ),
                Some(cookie),
                None,
                Some(json!({"body": body})),
            )
            .await;
        let (status, body) = json_of::<Value>(response).await;
        total += started.elapsed().as_secs_f64();
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    total * 1000.0 / count as f64
}

#[tokio::test]
#[ignore = "benchmark; run with --ignored --nocapture"]
async fn bench_import_and_formulas() {
    let rows: usize = std::env::var("VULSE_BENCH_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "bench@example.com", "bench").await;
    let customers_fields = json!([
        {"name":"code","type":"string"},
        {"name":"name","type":"string"}
    ]);
    let orders_fields = json!([
        {"name":"order_no","type":"string"},
        {"name":"customer","type":"string"},
        {"name":"product","type":"string"},
        {"name":"category","type":"string"},
        {"name":"amount","type":"number"},
        {"name":"qty","type":"number"},
        {"name":"ordered","type":"date"},
        {"name":"status","type":"string"},
        {"name":"city","type":"string"},
        {"name":"paid","type":"boolean"}
    ]);
    let customers = create_collection(
        &app,
        &cookie,
        "customers",
        "private",
        json!({"fields": customers_fields}),
    )
    .await;
    let orders = create_collection(
        &app,
        &cookie,
        "orders",
        "private",
        json!({"fields": orders_fields}),
    )
    .await;
    let response = import_csv(
        &app,
        Some(&cookie),
        &customers.id,
        &customers_csv(),
        true,
        None,
    )
    .await;
    let (status, result) = json_of::<ImportResult>(response).await;
    assert_eq!(status, StatusCode::OK);
    assert!(result.errors.is_empty(), "{:?}", result.errors);

    let csv = orders_csv(rows);
    println!("\n{rows} rows, {:.1} MB CSV", csv.len() as f64 / 1e6);
    for commit in [false, true] {
        let started = Instant::now();
        let response = import_csv(&app, Some(&cookie), &orders.id, &csv, commit, None).await;
        let (status, result) = json_of::<ImportResult>(response).await;
        assert_eq!(status, StatusCode::OK);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        println!(
            "import {}: {:.2} s",
            if commit { "commit " } else { "preview" },
            started.elapsed().as_secs_f64()
        );
    }

    let mut fields = orders_fields.as_array().unwrap().clone();
    fields.push(json!({"name":"line_total","type":"calculated","formula":"=[amount]*[qty]"}));
    let s = put_blueprint(&app, &cookie, &orders, json!({"fields": fields})).await;
    println!("orders + local formula:              {s:.2} s");

    let mut fields = customers_fields.as_array().unwrap().clone();
    fields.push(json!({"name":"revenue","type":"calculated","formula":"=SUMIFS([orders.amount],[orders.customer],[code])"}));
    let s = put_blueprint(&app, &cookie, &customers, json!({"fields": fields.clone()})).await;
    println!("customers + SUMIFS over orders:      {s:.2} s");

    fields.push(json!({"name":"order_count","type":"calculated","formula":"=COUNTIFS([orders.customer],[code])"}));
    let s = put_blueprint(&app, &cookie, &customers, json!({"fields": fields})).await;
    println!("customers + COUNTIFS over orders:    {s:.2} s");

    let mut fields = orders_fields.as_array().unwrap().clone();
    fields.push(json!({"name":"line_total","type":"calculated","formula":"=[amount]*[qty]"}));
    fields.push(json!({"name":"customer_name","type":"calculated","formula":"=XLOOKUP([customer],[customers.code],[customers.name])"}));
    let s = put_blueprint(&app, &cookie, &orders, json!({"fields": fields})).await;
    println!("orders + XLOOKUP into customers:     {s:.2} s");

    // Single-record edits with both formula sets in place. Each one ends by
    // refreshing the record table of the collection reading the edited one.
    let edits: usize = std::env::var("VULSE_BENCH_EDITS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let ms = edit_records(&app, &cookie, &orders.id, edits, |body, i| {
        body["amount"] = json!(body["amount"].as_f64().unwrap() + 1.0 + i as f64);
    })
    .await;
    println!("edit one order's amount:             {ms:.1} ms/edit");
    let ms = edit_records(&app, &cookie, &customers.id, edits, |body, i| {
        body["name"] = json!(format!("Renamed {i}"));
    })
    .await;
    println!("edit one customer's name:            {ms:.1} ms/edit");
    let ms = edit_records(&app, &cookie, &orders.id, edits, |body, i| {
        body["city"] = json!(format!("moved {i}"));
    })
    .await;
    println!("edit an order field nothing reads:   {ms:.1} ms/edit");

    // Records that reference another collection.
    let (_, page) = json_of::<Value>(
        app.call(
            "POST",
            &format!("/api/v1/collections/{}/query", customers.id),
            Some(&cookie),
            None,
            Some(json!({"limit":1})),
        )
        .await,
    )
    .await;
    let customer_id = page["items"][0]["id"].as_str().unwrap().to_string();
    let linked_fields = json!([
        {"name":"order_no","type":"string"},
        {"name":"customer","type":"reference","collection":"customers"},
        {"name":"amount","type":"number"}
    ]);
    let linked = create_collection(
        &app,
        &cookie,
        "linked",
        "private",
        json!({"fields": linked_fields}),
    )
    .await;
    let mut csv = String::from("order_no,customer,amount\n");
    for i in 0..rows {
        csv.push_str(&format!("ORD-{i},{customer_id},{}\n", i % 1000));
    }
    let started = Instant::now();
    let response = import_csv(&app, Some(&cookie), &linked.id, &csv, true, None).await;
    let (status, result) = json_of::<ImportResult>(response).await;
    assert_eq!(status, StatusCode::OK);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    println!(
        "import with references:              {:.2} s",
        started.elapsed().as_secs_f64()
    );
    let mut fields = linked_fields.as_array().unwrap().clone();
    fields.push(json!({"name":"vat","type":"calculated","formula":"=[amount]*0.25"}));
    let s = put_blueprint(&app, &cookie, &linked, json!({"fields": fields})).await;
    println!("references + local formula:          {s:.2} s");

    // Importing again with readers of `orders` in place.
    let started = Instant::now();
    let response = import_csv(
        &app,
        Some(&cookie),
        &orders.id,
        &orders_csv(rows / 10),
        true,
        None,
    )
    .await;
    let (status, result) = json_of::<ImportResult>(response).await;
    assert_eq!(status, StatusCode::OK);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    println!(
        "import {} more rows with formulas: {:.2} s",
        rows / 10,
        started.elapsed().as_secs_f64()
    );
}
