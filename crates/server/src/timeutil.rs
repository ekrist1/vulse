use chrono::{Duration, SecondsFormat, Utc};

pub const SESSION_SECONDS: i64 = 14 * 24 * 60 * 60;

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

pub fn session_expiry() -> String {
    (Utc::now() + Duration::seconds(SESSION_SECONDS)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn is_expired(expires_at: &str) -> bool {
    match chrono::DateTime::parse_from_rfc3339(expires_at) {
        Ok(expires_at) => expires_at < Utc::now(),
        Err(_) => true,
    }
}
