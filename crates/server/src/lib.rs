mod assets;
mod auth;
mod backup;
mod client_metadata;
mod config;
mod content;
mod convert;
mod db;
mod error;
mod events;
mod export;
mod history;
mod import;
mod lookups;
mod mcp;
mod migrate;
mod mutations;
mod oauth;
mod public;
mod records;
mod relationships;
mod rows;
mod security;
mod sql;
mod state;
mod store;
mod summary;
mod timeutil;
mod users;
#[cfg(feature = "embed-web")]
mod web;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post};
use tower_http::services::{ServeDir, ServeFile};

use crate::config::Config;
use crate::error::AppError;
use crate::security::Throttle;
use crate::state::{AppState, AuthSettings};

const JSON_BODY_LIMIT: usize = 1024 * 1024;

pub async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let config = Config::from_env()?;
    tokio::fs::create_dir_all(&config.asset_dir).await?;
    let db = config.open_db().await?;
    let conn = db.connect().await?;
    migrate::migrate(&conn).await?;
    store::purge_expired(&conn, &timeutil::now_secs()).await?;
    let setup_token = config.setup_token.clone().unwrap_or_else(auth::new_token);
    if store::count_users(&conn).await? == 0 {
        let base = config
            .public_url
            .clone()
            .unwrap_or_else(|| format!("http://{}", config.bind));
        tracing::warn!(
            "No accounts exist yet. Open {base}/setup and enter this setup token to create the first admin: {setup_token}"
        );
    }
    security::spawn_cleanup(db.clone());
    if let Some(schedule) = config.backup.clone() {
        tracing::info!(
            dir = %schedule.dir.display(),
            hours = schedule.every.as_secs() / 3600,
            keep = schedule.keep,
            "scheduled backups on"
        );
        backup::spawn_scheduled(db.clone(), config.asset_dir.clone(), schedule);
    }
    #[cfg(unix)]
    if config.turso_url.is_none() {
        backup::spawn_socket(db.clone(), config.asset_dir.clone(), &config.db_path);
    }
    let bind = config.bind.clone();
    if config.mcp && config.public_url.is_none() {
        tracing::warn!(
            "VULSE_MCP is on without VULSE_PUBLIC_URL; MCP clients must reach this server at http://{bind}"
        );
    }
    let base_url = config
        .public_url
        .clone()
        .unwrap_or_else(|| format!("http://{bind}"));
    let app = app(AppState {
        db,
        password_work: Arc::new(tokio::sync::Semaphore::new(4)),
        asset_dir: config.asset_dir,
        web_dist: config.web_dist,
        auth: Arc::new(AuthSettings {
            cookie_secure: config.cookie_secure,
            setup_token_hash: auth::hash_secret(&setup_token),
            trust_proxy: config.trust_proxy,
            public_origin: config.public_url,
            password_wait: Duration::from_secs(10),
            base_url,
        }),
        throttle: Arc::new(Throttle::default()),
        mcp: config.mcp,
        backup: config.turso_url.is_none().then(|| {
            Arc::new(backup::Settings {
                dir: config
                    .backup
                    .as_ref()
                    .map(|schedule| schedule.dir.clone())
                    .unwrap_or_else(|| {
                        std::path::absolute("backups").unwrap_or_else(|_| PathBuf::from("backups"))
                    }),
                schedule: config.backup.clone(),
            })
        }),
    });
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// `vulse backup [DIR] [--keep N]`: writes one backup and exits. `DIR`
/// defaults to `VULSE_BACKUP_DIR`, then `./backups`.
pub async fn backup_command(args: &[String]) -> anyhow::Result<()> {
    let mut dir = None;
    let mut keep = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--keep" => {
                let value = args.next().context("--keep needs a number")?;
                keep = Some(
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .context("--keep must be a whole number of at least 1")?,
                );
            }
            flag if flag.starts_with('-') => anyhow::bail!("unknown option '{flag}'"),
            path if dir.is_none() => dir = Some(PathBuf::from(path)),
            extra => anyhow::bail!("unexpected argument '{extra}'"),
        }
    }
    let config = Config::from_env()?;
    if config.turso_url.is_some() {
        anyhow::bail!("vulse backup copies a local database; Turso keeps its own backups of remote ones");
    }
    if !config.db_path.is_file() {
        anyhow::bail!(
            "no database at {}; set VULSE_DB_PATH to the database Vulse uses",
            config.db_path.display()
        );
    }
    let dir = std::path::absolute(
        dir.or_else(|| config.backup.as_ref().map(|b| b.dir.clone()))
            .unwrap_or_else(|| PathBuf::from("backups")),
    )?;
    let request = backup::Request { dir, keep };
    // A running server holds the database lock, so it makes the backup.
    #[cfg(unix)]
    if let Some(reply) = backup::request(&config.db_path, &request).await? {
        let Some(path) = reply.path else {
            anyhow::bail!(reply.error.unwrap_or_else(|| "backup failed".into()));
        };
        print_backup(&path, reply.assets, reply.bytes, reply.removed, keep);
        return Ok(());
    }
    let db = config.open_db().await.map_err(|e| {
        if format!("{e:#}").contains("locked") {
            anyhow::anyhow!(
                "Vulse is running but did not answer at {}; start it with this version of vulse, or stop it and run vulse backup again",
                backup::socket_path(&config.db_path).display()
            )
        } else {
            e
        }
    })?;
    let made = backup::create(&db, &config.asset_dir, &request.dir).await?;
    let removed = match keep {
        Some(keep) => backup::prune(&request.dir, keep).await?,
        None => 0,
    };
    print_backup(&made.path, made.assets, made.bytes, removed, keep);
    Ok(())
}

fn print_backup(path: &std::path::Path, assets: usize, bytes: u64, removed: usize, keep: Option<usize>) {
    println!(
        "Backup written to {} ({assets} uploaded files, {:.1} MB)",
        path.display(),
        bytes as f64 / 1_000_000.0
    );
    if let Some(keep) = keep
        && removed > 0
    {
        println!("Removed {removed} older backups, keeping the newest {keep}");
    }
}

pub fn app(state: AppState) -> Router {
    let web_dist = state.web_dist.clone();
    let mut api = api();
    if state.mcp {
        api = api.merge(oauth::api_routes());
    }
    let api = api.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        security::same_origin_writes,
    ));
    let mut router = Router::new().nest("/api/v1", api);
    if state.mcp {
        router = router.merge(mcp::router(&state)).merge(oauth::routes());
    }
    if web_dist.join("index.html").is_file() {
        let index = web_dist.join("index.html");
        let serve = ServeDir::new(&web_dist).fallback(ServeFile::new(index));
        router = router.fallback_service(serve);
    } else {
        #[cfg(feature = "embed-web")]
        if web::embedded() {
            router = router.fallback(web::serve);
        }
    }
    router
        .layer(axum::middleware::from_fn(security::page_headers))
        .with_state(state)
}

fn api() -> Router<AppState> {
    let json_limit = Router::new()
        .route("/auth/setup", get(auth::setup_status).post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/logout-everywhere", post(auth::logout_everywhere))
        .route("/auth/password", post(auth::change_password))
        .route("/auth/links/inspect", post(auth::link_info))
        .route("/auth/links/accept", post(auth::accept_link))
        .route("/me", get(auth::me))
        .route("/users", get(users::list_users))
        .route("/users/{id}", patch(users::update_user))
        .route("/users/{id}/password-reset", post(users::reset_password))
        .route(
            "/invites",
            get(users::list_invites).post(users::create_invite),
        )
        .route("/invites/{id}", delete(users::delete_invite))
        .route("/api-keys", get(auth::list_keys).post(auth::create_key))
        .route("/backups", get(backup::list_backups).post(backup::create_backup))
        .route(
            "/backups/{name}",
            get(backup::download_backup).delete(backup::delete_backup),
        )
        .route("/api-keys/{id}", delete(auth::delete_key))
        .route(
            "/collections",
            get(content::list_collections).post(content::create_collection),
        )
        .route(
            "/collections/{id}",
            get(content::get_collection)
                .patch(content::update_collection)
                .delete(content::delete_collection),
        )
        .route(
            "/collections/{id}/documents",
            get(content::list_documents).post(content::create_document),
        )
        .route(
            "/collections/{id}/documents/{doc_id}/references",
            get(relationships::backlinks),
        )
        .route("/collections/{id}/deleted", get(history::deleted))
        .route(
            "/collections/{id}/documents/{doc_id}/revisions",
            get(history::list),
        )
        .route(
            "/collections/{id}/documents/{doc_id}/revisions/{revision_id}/restore",
            post(history::restore),
        )
        .route("/sql", post(sql::run_sql))
        .route("/collections/{id}/query", post(records::query_records))
        .route("/collections/{id}/events", get(events::collection_events))
        .route("/collections/{id}/export", post(export::export_records))
        .route("/collections/{id}/summary", post(summary::summarize))
        .route(
            "/collections/{id}/views/{view_id}/summary",
            get(summary::view_summary),
        )
        .route("/collections/{id}/bulk", post(mutations::bulk_records))
        .route(
            "/collections/{id}/actions/{action_id}/undo",
            post(mutations::undo_action),
        )
        .route(
            "/collections/{id}/views",
            get(records::list_views).post(records::create_view),
        )
        .route(
            "/collections/{id}/views/{view_id}",
            axum::routing::put(records::update_view).delete(records::delete_view),
        )
        .route(
            "/collections/{id}/documents/{doc_id}",
            get(content::get_document)
                .put(content::update_document)
                .patch(mutations::patch_document)
                .delete(content::delete_document),
        )
        .route(
            "/public/{collection}",
            get(public::list_documents).options(public::options),
        )
        .route(
            "/public/{collection}/summary",
            post(public::summary).options(public::options),
        )
        .route(
            "/public/{collection}/{doc_id}",
            get(public::get_document).options(public::options),
        )
        .route(
            "/public/{collection}/assets/{asset_id}",
            get(public::get_asset).options(public::options),
        )
        .layer(DefaultBodyLimit::max(JSON_BODY_LIMIT));

    let imports = Router::new()
        .route("/collections/{id}/import", post(import::import_file))
        .route(
            "/collections/{id}/import/fields",
            post(import::suggest_fields),
        )
        .layer(DefaultBodyLimit::max(import::MAX_BYTES + 64 * 1024));

    let uploads = Router::new()
        .route(
            "/collections/{id}/assets",
            get(assets::list_assets).post(assets::upload_asset),
        )
        .route(
            "/collections/{id}/assets/{asset_id}",
            get(assets::get_asset).delete(assets::delete_asset),
        )
        .route(
            "/collections/{id}/assets/{asset_id}/content",
            get(assets::get_asset_content),
        )
        .layer(DefaultBodyLimit::max(
            assets::MAX_UPLOAD_BYTES + 1024 * 1024,
        ));

    json_limit.merge(uploads).merge(imports).fallback(not_found)
}

async fn not_found() -> AppError {
    AppError::NotFound
}

pub const TEST_SETUP_TOKEN: &str = "test-setup-token-0123456789";

pub async fn test_state(root: PathBuf) -> anyhow::Result<AppState> {
    let asset_dir = root.join("assets");
    tokio::fs::create_dir_all(&asset_dir).await?;
    let db = db::Db::open_local(&root.join("vulse.db")).await?;
    let conn = db.connect().await?;
    migrate::migrate(&conn).await?;
    Ok(AppState {
        db,
        password_work: Arc::new(tokio::sync::Semaphore::new(4)),
        asset_dir,
        web_dist: root.join("missing-dist"),
        auth: Arc::new(AuthSettings {
            cookie_secure: false,
            setup_token_hash: auth::hash_secret(TEST_SETUP_TOKEN),
            trust_proxy: false,
            public_origin: None,
            password_wait: Duration::from_millis(50),
            base_url: "http://localhost".into(),
        }),
        throttle: Arc::new(Throttle::default()),
        mcp: true,
        backup: Some(Arc::new(backup::Settings {
            dir: root.join("backups"),
            schedule: None,
        })),
    })
}

#[cfg(test)]
mod tests;
