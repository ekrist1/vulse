use std::path::PathBuf;

use anyhow::Context;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub turso_url: Option<String>,
    pub turso_token: Option<String>,
    pub db_path: PathBuf,
    pub asset_dir: PathBuf,
    pub web_dist: PathBuf,
    /// External address of the admin, such as `https://cms.example.com`.
    pub public_url: Option<String>,
    pub cookie_secure: bool,
    pub setup_token: Option<String>,
    pub trust_proxy: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let turso_url = nonempty("TURSO_DATABASE_URL");
        let turso_token = nonempty("TURSO_AUTH_TOKEN");
        match (&turso_url, &turso_token) {
            (Some(_), None) => {
                anyhow::bail!("TURSO_AUTH_TOKEN is required when TURSO_DATABASE_URL is set")
            }
            (None, Some(_)) => {
                anyhow::bail!("TURSO_DATABASE_URL is required when TURSO_AUTH_TOKEN is set")
            }
            _ => {}
        }
        let public_url =
            nonempty("VULSE_PUBLIC_URL").map(|url| url.trim_end_matches('/').to_string());
        if let Some(url) = &public_url
            && !url.starts_with("https://") && !url.starts_with("http://") {
                anyhow::bail!("VULSE_PUBLIC_URL must start with https:// or http://");
            }
        let cookie_secure = match nonempty("VULSE_COOKIE_SECURE") {
            Some(value) => flag("VULSE_COOKIE_SECURE", &value)?,
            None => public_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://")),
        };
        let trust_proxy = match nonempty("VULSE_TRUST_PROXY") {
            Some(value) => flag("VULSE_TRUST_PROXY", &value)?,
            None => false,
        };
        let setup_token = nonempty("VULSE_SETUP_TOKEN");
        if setup_token.as_ref().is_some_and(|token| token.len() < 16) {
            anyhow::bail!("VULSE_SETUP_TOKEN must be at least 16 characters");
        }
        Ok(Self {
            bind: std::env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8080".into()),
            turso_url,
            turso_token,
            db_path: std::env::var("VULSE_DB_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("vulse.db")),
            asset_dir: std::env::var("ASSET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("storage")),
            web_dist: std::env::var("VULSE_WEB_DIST")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("crates/web/dist")),
            public_url,
            cookie_secure,
            setup_token,
            trust_proxy,
        })
    }

    pub async fn open_db(&self) -> anyhow::Result<crate::db::Db> {
        if let Some(url) = &self.turso_url {
            let token = self
                .turso_token
                .as_deref()
                .context("missing Turso auth token")?;
            crate::db::Db::open_remote(url, token).await
        } else {
            if let Some(parent) = self.db_path.parent()
                && !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent).await?;
                }
            crate::db::Db::open_local(&self.db_path).await
        }
    }
}

fn nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn flag(name: &str, value: &str) -> anyhow::Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("{name} must be true or false"),
    }
}
