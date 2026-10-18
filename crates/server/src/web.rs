//! The admin UI built into the binary by the `embed-web` feature, so a
//! release is one file. A `VULSE_WEB_DIST` directory with an `index.html`
//! still takes precedence, for development against a fresh Trunk build.
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../web/dist"]
struct Dist;

/// Whether the binary was built with the admin UI inside it.
pub fn embedded() -> bool {
    Dist::get("index.html").is_some()
}

/// Serves an embedded file, or `index.html` for client-side routes.
pub async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = uri.path().trim_start_matches('/');
    let (file, path) = match Dist::get(path).filter(|_| !path.is_empty()) {
        Some(file) => (file, path),
        None => match Dist::get("index.html") {
            Some(file) => (file, "index.html"),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
    };
    let etag = format!("\"{}\"", hex::encode(file.metadata.sha256_hash()));
    // Every response revalidates; unchanged files cost a 304 without a body.
    let mut response = if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|value| value.as_bytes() == etag.as_bytes())
    {
        StatusCode::NOT_MODIFIED.into_response()
    } else if method == Method::HEAD {
        Response::new(Body::empty())
    } else {
        Response::new(Body::from(file.data.into_owned()))
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}
