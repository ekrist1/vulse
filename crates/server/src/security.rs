use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts, Request};
use axum::http::Method;
use axum::http::header::{self, HeaderValue};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::state::AppState;

/// Failed attempts are counted over this window.
pub const WINDOW: Duration = Duration::from_secs(15 * 60);
/// Failed sign-ins allowed per account in one window.
pub const ACCOUNT_LIMIT: usize = 5;
/// Failed sign-ins, setup attempts and link lookups allowed per client address.
pub const ADDRESS_LIMIT: usize = 30;
const MAX_TRACKED: usize = 10_000;

/// In-memory failure counter for authentication endpoints. Counts reset when
/// the process restarts and are not shared between instances; put a limiter at
/// the ingress as well when running more than one.
#[derive(Default)]
pub struct Throttle {
    failures: Mutex<HashMap<String, Vec<Instant>>>,
}

impl Throttle {
    /// Refuses when any key has used up its limit, reporting when the oldest
    /// counted failure leaves the window.
    pub fn check(&self, keys: &[(&str, usize)]) -> Result<(), AppError> {
        let now = Instant::now();
        let mut failures = self.failures.lock().expect("throttle lock");
        let mut wait = Duration::ZERO;
        for (key, limit) in keys {
            if let Some(times) = failures.get_mut(*key) {
                times.retain(|time| now.duration_since(*time) < WINDOW);
                if times.len() >= *limit {
                    let oldest = times[times.len() - limit];
                    wait = wait.max(WINDOW.saturating_sub(now.duration_since(oldest)));
                }
            }
        }
        if wait.is_zero() {
            Ok(())
        } else {
            Err(AppError::RateLimited {
                retry_after: wait.as_secs().max(1),
            })
        }
    }

    pub fn fail(&self, keys: &[&str]) {
        let now = Instant::now();
        let mut failures = self.failures.lock().expect("throttle lock");
        if failures.len() >= MAX_TRACKED {
            failures.retain(|_, times| {
                times.retain(|time| now.duration_since(*time) < WINDOW);
                !times.is_empty()
            });
        }
        for key in keys {
            failures.entry((*key).to_string()).or_default().push(now);
        }
    }

    pub fn clear(&self, key: &str) {
        self.failures.lock().expect("throttle lock").remove(key);
    }
}

pub fn account_key(email: &str) -> String {
    format!("account:{email}")
}

pub fn address_key(ip: &str) -> String {
    format!("address:{ip}")
}

/// Client address used for throttling.
pub struct ClientIp(pub String);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if state.auth.trust_proxy {
            // The trusted proxy appends the address it saw, so the last entry
            // is the only one a client cannot forge.
            let forwarded = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.rsplit(',').map(str::trim).find(|v| !v.is_empty()));
            if let Some(ip) = forwarded {
                return Ok(ClientIp(ip.to_string()));
            }
        }
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip().to_string())
            .unwrap_or_else(|| "unknown".into());
        Ok(ClientIp(ip))
    }
}

/// Rejects cookie-authenticated writes that a browser reports as coming from
/// another site. Requests with an `Authorization` header are not sent
/// automatically by browsers and pass through.
pub async fn same_origin_writes(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method();
    let safe = matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS);
    let headers = request.headers();
    // Public endpoints ignore cookies, so cross-site calls are expected there.
    let path = request.uri().path();
    let public = path.starts_with("/public/") || path.starts_with("/api/v1/public/");
    if safe || public || headers.contains_key(header::AUTHORIZATION) {
        return next.run(request).await;
    }
    let fetch_site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok());
    let allowed = match fetch_site {
        Some(site) => matches!(site, "same-origin" | "none"),
        None => match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            None => true,
            Some(origin) => {
                let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
                let authority = origin.split_once("://").map(|(_, rest)| rest);
                (authority.is_some() && authority == host)
                    || state.auth.public_origin.as_deref() == Some(origin)
            }
        },
    };
    if allowed {
        next.run(request).await
    } else {
        AppError::Forbidden("cross-site requests are not allowed".into()).into_response()
    }
}

/// Headers for the admin pages. API responses set their own.
pub async fn page_headers(request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    if !is_api {
        let headers = response.headers_mut();
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("frame-ancestors 'none'"),
        );
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    }
    response
}

/// Deletes expired sessions and account links every hour.
pub fn spawn_cleanup(db: crate::db::Db) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            interval.tick().await;
            let result = async {
                let conn = db.connect().await?;
                crate::store::purge_expired(&conn, &crate::timeutil::now_secs()).await
            }
            .await;
            match result {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "removed expired sessions and links"),
                Err(err) => tracing::warn!(error = %err, "session cleanup failed"),
            }
        }
    });
}
