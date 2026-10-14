//! Read performance on a large collection. Ignored by default; run with
//!
//! ```bash
//! VULSE_BENCH_ROWS=300000 cargo test --release -p vulse-server bench_large_collection -- --ignored --nocapture
//! ```
//!
//! Records are written through the normal record path (record table,
//! history), and a second collection adds rows to the shared tables.

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{TestApp, create_collection, register};

/// Deterministic pseudo-random numbers, so runs are comparable.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const WORDS: [&str; 16] = [
    "alpha", "birch", "cobalt", "delta", "ember", "fjord", "granite", "harbor", "island",
    "juniper", "kelp", "lumen", "meadow", "north", "orchid", "pine",
];

fn record(rng: &mut Lcg, i: usize) -> Value {
    let title = format!(
        "{} {} {i}",
        WORDS[rng.below(16) as usize],
        WORDS[rng.below(16) as usize]
    );
    let mut body = json!({
        "title": title,
        "category": format!("category {}", rng.below(20)),
        "city": format!("city {}", rng.below(200)),
        "amount": rng.below(1_000_000) as f64 / 100.0,
        "year": 2000 + rng.below(26),
        "due": format!("20{:02}-{:02}-{:02}", rng.below(26), 1 + rng.below(12), 1 + rng.below(28)),
        "active": rng.below(2) == 1,
    });
    // Some records leave optional fields empty.
    if rng.below(10) == 0 {
        body.as_object_mut().unwrap().remove("city");
    }
    body
}

fn blueprint() -> Value {
    json!({"fields":[
        {"name":"title","type":"string","required":true},
        {"name":"category","type":"string"},
        {"name":"city","type":"string"},
        {"name":"amount","type":"number"},
        {"name":"year","type":"number"},
        {"name":"due","type":"date"},
        {"name":"active","type":"boolean"},
        {"name":"total","type":"calculated","formula":"=[amount]*1.25"}
    ]})
}

async fn seed(app: &TestApp, collection_id: &str, rows: usize, seed: u64) -> Duration {
    let started = Instant::now();
    let mut rng = Lcg(seed);
    let conn = app.db.connect().await.unwrap();
    let mut i = 0;
    while i < rows {
        let end = (i + 2000).min(rows);
        let bodies: Vec<_> = (i..end).map(|n| record(&mut rng, n)).collect();
        let first = i;
        conn.with_tx(async |conn| {
            for (n, body) in bodies.iter().enumerate() {
                // Distinct creation times keep the default order meaningful.
                let created = format!("2026-01-01T00:00:00.{:06}Z", first + n);
                crate::store::insert_document(
                    conn,
                    &crate::auth::new_id(),
                    collection_id,
                    body,
                    &created,
                )
                .await?;
            }
            Ok(())
        })
        .await
        .unwrap();
        i = end;
    }
    started.elapsed()
}

async fn time(app: &TestApp, cookie: &str, path: &str, body: Value) -> (Duration, Duration) {
    let mut runs = Vec::new();
    for _ in 0..3 {
        let started = Instant::now();
        let response = app
            .call("POST", path, Some(cookie), None, Some(body.clone()))
            .await;
        let status = response.status();
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        runs.push(started.elapsed());
        assert_eq!(
            status,
            StatusCode::OK,
            "{path} {body}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    runs.sort();
    (runs[0], runs[1])
}

fn ms(d: Duration) -> String {
    format!("{:.0}", d.as_secs_f64() * 1000.0)
}

#[tokio::test]
#[ignore = "benchmark; run with --ignored --nocapture"]
async fn bench_large_collection() {
    let rows: usize = std::env::var("VULSE_BENCH_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "bench@example.com", "bench").await;
    let main = create_collection(&app, &cookie, "bench", "private", blueprint()).await;
    let other = create_collection(&app, &cookie, "other", "private", blueprint()).await;
    let seeded = seed(&app, &main.id, rows, 1).await;
    let other_seeded = seed(&app, &other.id, rows / 4, 2).await;
    println!(
        "\nseeded {rows} records in {:.1} s ({:.0} records/s), plus {} in another collection in {:.1} s",
        seeded.as_secs_f64(),
        rows as f64 / seeded.as_secs_f64(),
        rows / 4,
        other_seeded.as_secs_f64()
    );
    let db_size = std::fs::metadata(app._dir.path().join("vulse.db"))
        .map(|m| m.len())
        .unwrap_or(0);
    println!("database file: {:.0} MB", db_size as f64 / 1e6);

    let query = format!("/api/v1/collections/{}/query", main.id);
    let summary = format!("/api/v1/collections/{}/summary", main.id);
    let middle = rows / 2;
    let cases: Vec<(&str, &str, Value)> = vec![
        ("newest first, page 1", &query, json!({"limit":25})),
        (
            "newest first, middle page",
            &query,
            json!({"limit":25,"offset":middle}),
        ),
        (
            "sort text (title), page 1",
            &query,
            json!({"sort_field":"title","descending":false,"limit":25}),
        ),
        (
            "sort text (title), middle page",
            &query,
            json!({"sort_field":"title","descending":false,"limit":25,"offset":middle}),
        ),
        (
            "sort number (amount) desc",
            &query,
            json!({"sort_field":"amount","descending":true,"limit":25}),
        ),
        (
            "sort calculated (total)",
            &query,
            json!({"sort_field":"total","limit":25}),
        ),
        (
            "sort date (due)",
            &query,
            json!({"sort_field":"due","limit":25}),
        ),
        (
            "group category, sort amount",
            &query,
            json!({"group_field":"category","sort_field":"amount","limit":25}),
        ),
        (
            "filter category = x",
            &query,
            json!({"filters":[{"field":"category","op":"eq","value":"category 7"}],"limit":25}),
        ),
        (
            "filter category = x, sort amount",
            &query,
            json!({"filters":[{"field":"category","op":"eq","value":"category 7"}],"sort_field":"amount","limit":25}),
        ),
        (
            "filter amount > 9000",
            &query,
            json!({"filters":[{"field":"amount","op":"gt","value":9000}],"limit":25}),
        ),
        (
            "filter year = 2010 and city = x",
            &query,
            json!({"filters":[{"field":"year","op":"eq","value":2010},{"field":"city","op":"eq","value":"city 42"}],"limit":25}),
        ),
        (
            "search 'juniper'",
            &query,
            json!({"search":"juniper","limit":25}),
        ),
        (
            "summary: count by category",
            &summary,
            json!({"summary":{"group_by":[{"field":"category"}],"measures":[{"name":"n","op":"count"}]}}),
        ),
        (
            "summary: sum amount by year",
            &summary,
            json!({"summary":{"group_by":[{"field":"year"}],"measures":[{"name":"s","op":"sum","field":"amount"}]}}),
        ),
        (
            "summary: sum by category and year",
            &summary,
            json!({"summary":{"group_by":[{"field":"category"},{"field":"year"}],"measures":[{"name":"s","op":"sum","field":"amount"},{"name":"n","op":"count"}]}}),
        ),
        (
            "SQL: count where amount > 9000",
            "/api/v1/sql",
            json!({"sql":"SELECT COUNT(*) FROM bench WHERE amount > 9000"}),
        ),
        (
            "SQL: sum by category",
            "/api/v1/sql",
            json!({"sql":"SELECT category, SUM(amount) FROM bench GROUP BY category"}),
        ),
        (
            "SQL: year = 2010 and city = x",
            "/api/v1/sql",
            json!({"sql":"SELECT title, amount FROM bench WHERE year = 2010 AND city = 'city 42'"}),
        ),
        (
            "SQL: top 10 by amount",
            "/api/v1/sql",
            json!({"sql":"SELECT title, amount FROM bench ORDER BY amount DESC LIMIT 10"}),
        ),
        (
            "SQL: select * where title like",
            "/api/v1/sql",
            json!({"sql":"SELECT * FROM bench WHERE title LIKE 'juniper pine%'"}),
        ),
        (
            "SQL: update preview, one category",
            "/api/v1/sql",
            json!({"sql":"UPDATE bench SET active = false WHERE category = 'category 7' AND year < 2002"}),
        ),
        (
            "SQL: delete preview, amount < 10",
            "/api/v1/sql",
            json!({"sql":"DELETE FROM bench WHERE amount < 10"}),
        ),
    ];
    let conn = app.db.connect().await.unwrap();
    for table in ["documents", "record_revisions"] {
        if let Ok(rows) = conn
            .query(&format!("SELECT count(*) FROM {table}"), &[])
            .await
        {
            println!("{table}: {} rows", crate::db::int_at(&rows[0], 0).unwrap());
        }
    }
    println!("\n{:<36} {:>9} {:>9}", "case", "best ms", "median ms");
    for (name, path, body) in cases {
        let (best, median) = time(&app, &cookie, path, body).await;
        println!("{name:<36} {:>9} {:>9}", ms(best), ms(median));
    }
    // Changing fields rewrites the record table from every record body.
    let mut fields = blueprint();
    fields["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"note","type":"string"}));
    let started = Instant::now();
    let response = app
        .call(
            "PATCH",
            &format!("/api/v1/collections/{}", main.id),
            Some(&cookie),
            None,
            Some(json!({"blueprint": fields})),
        )
        .await;
    let status = response.status();
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    println!(
        "\nadd a field (rebuilds the table)      {:>9}",
        ms(started.elapsed())
    );
}
