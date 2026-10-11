use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;
use crate::security::Throttle;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub password_work: Arc<tokio::sync::Semaphore>,
    pub asset_dir: PathBuf,
    pub web_dist: PathBuf,
    pub auth: Arc<AuthSettings>,
    pub throttle: Arc<Throttle>,
}

pub struct AuthSettings {
    /// Adds `Secure` and the `__Host-` cookie prefix.
    pub cookie_secure: bool,
    /// SHA-256 of the one-time token that creates the first admin.
    pub setup_token_hash: String,
    /// Read the client address from `X-Forwarded-For`.
    pub trust_proxy: bool,
    /// Extra origin accepted for cookie-authenticated writes.
    pub public_origin: Option<String>,
    /// How long a sign-in waits for a free password hashing slot.
    pub password_wait: Duration,
}

impl AuthSettings {
    pub fn cookie_name(&self) -> &'static str {
        if self.cookie_secure {
            "__Host-vulse_session"
        } else {
            "vulse_session"
        }
    }
}
