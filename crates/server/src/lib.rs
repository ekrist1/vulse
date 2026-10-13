mod assets;
mod auth;
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
mod migrate;
mod mutations;
mod public;
mod records;
mod relationships;
mod security;
mod sql;
mod state;
mod store;
mod summary;
mod timeutil;
mod users;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
    let bind = config.bind.clone();
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
        }),
        throttle: Arc::new(Throttle::default()),
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

pub fn app(state: AppState) -> Router {
    let web_dist = state.web_dist.clone();
    let api = api().layer(axum::middleware::from_fn_with_state(
        state.clone(),
        security::same_origin_writes,
    ));
    let mut router = Router::new().nest("/api/v1", api);
    if web_dist.join("index.html").is_file() {
        let index = web_dist.join("index.html");
        let serve = ServeDir::new(&web_dist).fallback(ServeFile::new(index));
        router = router.fallback_service(serve);
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
        }),
        throttle: Arc::new(Throttle::default()),
    })
}

#[cfg(test)]
mod tests;
