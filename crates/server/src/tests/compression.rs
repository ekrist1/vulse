//! Response compression: what is compressed, what is left alone, and what
//! it saves on a page of records.
use std::io::Read;

use super::mcp::{MCP_HEADERS, api_key};
use super::*;

const BOTH: [(&str, &str); 1] = [("accept-encoding", "gzip, br")];
const GZIP: [(&str, &str); 1] = [("accept-encoding", "gzip")];

fn encoding(response: &axum::response::Response) -> Option<&str> {
    response
        .headers()
        .get("content-encoding")
        .map(|value| value.to_str().unwrap())
}

async fn bytes_of(response: axum::response::Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

fn decode(encoding: Option<&str>, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    match encoding {
        None => out.extend_from_slice(bytes),
        Some("gzip") => {
            flate2::read::GzDecoder::new(bytes)
                .read_to_end(&mut out)
                .unwrap();
        }
        Some("br") => {
            brotli_decompressor::Decompressor::new(bytes, 4096)
                .read_to_end(&mut out)
                .unwrap();
        }
        Some(other) => panic!("unexpected content-encoding {other}"),
    }
    out
}

/// Fetches `uri` without and with each encoding, checks every variant
/// decodes to the same bytes, and returns the sizes (identity, gzip, br).
async fn sizes(app: &TestApp, uri: &str, cookie: Option<&str>) -> (usize, usize, usize) {
    let plain = app.call("GET", uri, cookie, None, None).await;
    assert_eq!(plain.status(), StatusCode::OK, "{uri}");
    assert_eq!(encoding(&plain), None, "{uri}");
    // Caches must keep the variants apart.
    assert_eq!(plain.headers()["vary"], "accept-encoding", "{uri}");
    let plain = bytes_of(plain).await;
    let mut sizes = vec![plain.len()];
    for name in ["gzip", "br"] {
        let response = app
            .call_with("GET", uri, cookie, None, None, &[("accept-encoding", name)])
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert_eq!(encoding(&response), Some(name), "{uri}");
        assert!(response.headers().get("content-length").is_none());
        let bytes = bytes_of(response).await;
        assert_eq!(decode(Some(name), &bytes), plain, "{uri} {name}");
        sizes.push(bytes.len());
    }
    (sizes[0], sizes[1], sizes[2])
}

#[tokio::test]
async fn json_pages_are_compressed_when_the_client_accepts_it() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let collection = create_collection(
        &app,
        &cookie,
        "orders",
        "private",
        json!({"fields":[
            {"name":"customer","type":"string","required":true},
            {"name":"email","type":"string"},
            {"name":"status","type":"string","options":["open","paid","shipped","cancelled"]},
            {"name":"amount","type":"number"},
            {"name":"ordered","type":"date"},
            {"name":"notes","type":"string"},
            {"name":"total","type":"calculated","formula":"=[amount]*1.25"}
        ]}),
    )
    .await;
    let statuses = ["open", "paid", "shipped", "cancelled"];
    let names = [
        "Ada Lovelace",
        "Grace Hopper",
        "Alan Turing",
        "Edsger Dijkstra",
        "Barbara Liskov",
    ];
    for i in 0..100usize {
        let name = names[i % names.len()];
        create_document(
            &app,
            &cookie,
            &collection.id,
            json!({
                "customer": format!("{name} {i}"),
                "email": format!("customer{i}@example.com"),
                "status": statuses[i % statuses.len()],
                "amount": (i * 37 % 1000) as f64 + 0.5,
                "ordered": format!("2026-{:02}-{:02}", i % 12 + 1, i % 28 + 1),
                "notes": format!("Order {i}: deliver to the back door, call ahead if nobody answers."),
            }),
        )
        .await;
    }
    let uri = format!("/api/v1/collections/{}/documents?limit=100", collection.id);
    let (plain, gzip, br) = sizes(&app, &uri, Some(&cookie)).await;
    println!(
        "100 records: {plain} bytes, gzip {gzip} ({:.0}% smaller), br {br} ({:.0}% smaller)",
        100.0 - gzip as f64 * 100.0 / plain as f64,
        100.0 - br as f64 * 100.0 / plain as f64,
    );
    assert!(gzip * 4 < plain && br * 4 < plain, "{plain} {gzip} {br}");

    // Brotli wins when the client offers both.
    let response = app
        .call_with("GET", &uri, Some(&cookie), None, None, &BOTH)
        .await;
    assert_eq!(encoding(&response), Some("br"));

    // Tiny responses are not worth the framing.
    let response = app
        .call_with("GET", "/api/v1/me", Some(&cookie), None, None, &BOTH)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(encoding(&response), None);
}

#[tokio::test]
async fn admin_files_from_disk_are_compressed() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = crate::test_state(dir.path().to_path_buf()).await.unwrap();
    let dist = dir.path().join("dist");
    std::fs::create_dir_all(&dist).unwrap();
    let index = format!(
        "<!doctype html><html><head>{}</head><body></body></html>",
        "<link rel=\"modulepreload\" href=\"/app.js\">".repeat(40)
    );
    std::fs::write(dist.join("index.html"), &index).unwrap();
    std::fs::write(
        dist.join("app.js"),
        "export function run() { return 1; }\n".repeat(500),
    )
    .unwrap();
    // Stands in for the WASM bundle: binary, but full of repetition.
    let wasm: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 251) as u8 / 4).collect();
    std::fs::write(dist.join("app.wasm"), &wasm).unwrap();
    std::fs::write(dist.join("logo.png"), vec![7u8; 4096]).unwrap();
    state.web_dist = dist;
    let app = TestApp::with_state(state, dir);

    for uri in [
        "/app.wasm",
        "/app.js",
        "/",
        "/collections/some-client-route",
    ] {
        let (plain, gzip, br) = sizes(&app, uri, None).await;
        assert!(gzip < plain && br < plain, "{uri}: {plain} {gzip} {br}");
    }
    let response = app
        .call_with("GET", "/app.wasm", None, None, None, &BOTH)
        .await;
    assert_eq!(response.headers()["content-type"], "application/wasm");

    // Images are already compressed.
    let response = app
        .call_with("GET", "/logo.png", None, None, None, &BOTH)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(encoding(&response), None);

    // Ranges are served as stored bytes, never compressed.
    let response = app
        .call_with(
            "GET",
            "/app.wasm",
            None,
            None,
            None,
            &[("accept-encoding", "gzip, br"), ("range", "bytes=0-99")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(encoding(&response), None);
    assert_eq!(bytes_of(response).await, wasm[..100]);

    // Revalidation still answers 304 without a body.
    let response = app
        .call_with("GET", "/app.wasm", None, None, None, &BOTH)
        .await;
    let modified = response.headers()["last-modified"]
        .to_str()
        .unwrap()
        .to_string();
    let response = app
        .call_with(
            "GET",
            "/app.wasm",
            None,
            None,
            None,
            &[
                ("accept-encoding", "gzip, br"),
                ("if-modified-since", &modified),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(encoding(&response), None);
    assert!(bytes_of(response).await.is_empty());
}

/// Only runs with `--features embed-web` and a built `crates/web/dist`.
#[cfg(feature = "embed-web")]
#[tokio::test]
async fn embedded_admin_files_are_compressed_and_revalidate() {
    let app = TestApp::new().await;
    let response = app.call("GET", "/", None, None, None).await;
    if response.status() == StatusCode::NOT_FOUND {
        eprintln!("crates/web/dist is not built; skipping");
        return;
    }
    let index = String::from_utf8(bytes_of(response).await).unwrap();
    let mut files: Vec<&str> = index
        .split(['"', '\''])
        .filter(|part| {
            part.starts_with('/')
                && [".js", ".css", ".wasm"]
                    .iter()
                    .any(|ext| part.ends_with(ext))
        })
        .collect();
    files.sort();
    files.dedup();
    assert!(
        files.iter().any(|file| file.ends_with("_bg.wasm")),
        "{files:?}"
    );
    for uri in files {
        let started = std::time::Instant::now();
        let (plain, gzip, br) = sizes(&app, uri, None).await;
        println!(
            "{uri}: {plain} bytes, gzip {gzip}, br {br} ({:.0}% smaller), three fetches in {:?}",
            100.0 - br as f64 * 100.0 / plain as f64,
            started.elapsed()
        );
        assert!(br < plain && gzip < plain);

        // The compressed variant has its own validator, and a client
        // holding it gets a 304 without a body.
        let response = app.call_with("GET", uri, None, None, None, &BOTH).await;
        assert_eq!(encoding(&response), Some("br"));
        let etag = response.headers()["etag"].to_str().unwrap().to_string();
        assert!(etag.starts_with("W/"), "{etag}");
        for (accept, sent) in [
            ("gzip, br", etag.as_str()),
            ("", etag.trim_start_matches("W/")),
        ] {
            let response = app
                .call_with(
                    "GET",
                    uri,
                    None,
                    None,
                    None,
                    &[("accept-encoding", accept), ("if-none-match", sent)],
                )
                .await;
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{sent}");
            assert_eq!(encoding(&response), None);
            assert!(bytes_of(response).await.is_empty());
        }
    }
}

#[tokio::test]
async fn change_events_are_not_compressed() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let collection = create_collection(
        &app,
        &cookie,
        "notes",
        "private",
        json!({"fields":[{"name":"title","type":"string"}]}),
    )
    .await;
    let response = app
        .call_with(
            "GET",
            &format!("/api/v1/collections/{}/events", collection.id),
            Some(&cookie),
            None,
            None,
            &BOTH,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(encoding(&response), None);
    // Each event arrives as soon as it is published, not when a buffer fills.
    let mut body = response.into_body();
    create_document(&app, &cookie, &collection.id, json!({"title":"a"})).await;
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
            .await
            .expect("an event within 5 seconds")
            .unwrap()
            .unwrap();
        if let Ok(data) = frame.into_data()
            && data.starts_with(b"event: change")
        {
            assert!(String::from_utf8_lossy(&data).contains(&collection.id));
            break;
        }
    }
}

#[tokio::test]
async fn mcp_replies_are_not_compressed() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let key = api_key(&app, &cookie, json!({"name":"agent"})).await;
    let mut headers = MCP_HEADERS.to_vec();
    headers.push(("accept-encoding", "gzip, br"));
    let response = app
        .call_with(
            "POST",
            "/mcp",
            None,
            Some(&key),
            Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})),
            &headers,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(encoding(&response), None);
    let bytes = bytes_of(response).await;
    assert!(bytes.len() > 1024, "tools/list is big enough to compress");
    let reply: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        reply["result"]["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty())
    );
}

#[tokio::test]
async fn uploads_and_backups_are_sent_as_stored() {
    let app = TestApp::new().await;
    let (cookie, _) = register(&app, "ada@example.com", "ada").await;
    let collection = create_collection(
        &app,
        &cookie,
        "papers",
        "public",
        json!({"fields":[{"name":"file","type":"asset"}]}),
    )
    .await;
    // Large and repetitive, so only the content type keeps it uncompressed.
    let pdf = b"%PDF-1.7\n".repeat(1000);
    let png = [b"\x89PNG\r\n".as_slice(), &[0u8; 8000]].concat();
    for (content_type, bytes, name) in [
        ("application/pdf", pdf.as_slice(), "paper.pdf"),
        ("image/png", png.as_slice(), "figure.png"),
        ("video/mp4", pdf.as_slice(), "clip.mp4"),
        ("audio/mpeg", pdf.as_slice(), "talk.mp3"),
    ] {
        let uploaded =
            multipart_call(&app, &cookie, &collection.id, content_type, bytes, name).await;
        let (status, asset) = json_of::<AssetView>(uploaded).await;
        assert_eq!(status, StatusCode::CREATED);
        create_document(&app, &cookie, &collection.id, json!({"file": asset.id})).await;
        for (uri, cookie) in [
            (
                format!(
                    "/api/v1/collections/{}/assets/{}/content",
                    collection.id, asset.id
                ),
                Some(cookie.as_str()),
            ),
            (format!("/api/v1/public/papers/assets/{}", asset.id), None),
        ] {
            let response = app.call_with("GET", &uri, cookie, None, None, &BOTH).await;
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            assert_eq!(response.headers()["content-type"], content_type);
            assert_eq!(encoding(&response), None, "{uri}");
            assert_eq!(bytes_of(response).await, bytes, "{uri}");
        }
    }

    // Backups are zip files, already deflated.
    let (status, made) = json_of::<vulse_model::BackupView>(
        app.call("POST", "/api/v1/backups", Some(&cookie), None, None)
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let download = app
        .call_with(
            "GET",
            &format!("/api/v1/backups/{}", made.name),
            Some(&cookie),
            None,
            None,
            &GZIP,
        )
        .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(encoding(&download), None);
    assert_eq!(
        download.headers()["content-length"],
        made.bytes.to_string().as_str()
    );
    assert_eq!(bytes_of(download).await.len() as u64, made.bytes);
}
