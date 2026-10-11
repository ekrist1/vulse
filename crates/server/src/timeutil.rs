use chrono::{Duration, SecondsFormat, Utc};

pub const SESSION_SECONDS: i64 = 14 * 24 * 60 * 60;
pub const INVITE_SECONDS: i64 = 72 * 60 * 60;
pub const RESET_SECONDS: i64 = 24 * 60 * 60;

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// Fixed-width UTC timestamp used for expiry columns, so SQL text comparison
/// orders them correctly.
pub fn now_secs() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn expiry_in(seconds: i64) -> String {
    (Utc::now() + Duration::seconds(seconds)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn session_expiry() -> String {
    expiry_in(SESSION_SECONDS)
}

pub fn is_expired(expires_at: &str) -> bool {
    match chrono::DateTime::parse_from_rfc3339(expires_at) {
        Ok(expires_at) => expires_at < Utc::now(),
        Err(_) => true,
    }
}
