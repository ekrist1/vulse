mod assets;
mod auth;
mod config;
mod content;
mod db;
mod error;
mod export;
mod history;
mod import;
mod migrate;
mod mutations;
mod public;
mod records;
mod relationships;
mod state;
mod store;
mod timeutil;

use std::path::PathBuf;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use tower_http::services::{ServeDir, ServeFile};

use crate::config::Config;
use crate::error::AppError;
use crate::state::AppState;

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
    let bind = config.bind.clone();
    let app = app(AppState {
        db,
        password_work: std::sync::Arc::new(tokio::sync::Semaphore::new(4)),
        asset_dir: config.asset_dir,
        web_dist: config.web_dist,
    });
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn app(state: AppState) -> Router {
    let web_dist = state.web_dist.clone();
    let mut router = Router::new().nest("/api/v1", api());
    if web_dist.join("index.html").is_file() {
        let index = web_dist.join("index.html");
        let serve = ServeDir::new(&web_dist).fallback(ServeFile::new(index));
        router = router.fallback_service(serve);
    }
    router.with_state(state)
}

fn api() -> Router<AppState> {
    let json_limit = Router::new()
        .route("/auth/register", post(auth::register))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/me", get(auth::me))
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
        .route("/collections/{id}/query", post(records::query_records))
        .route("/collections/{id}/export", post(export::export_records))
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
            "/public/{handle}/{collection}",
            get(public::list_documents).options(public::options),
        )
        .route(
            "/public/{handle}/{collection}/{doc_id}",
            get(public::get_document).options(public::options),
        )
        .route(
            "/public/{handle}/{collection}/assets/{asset_id}",
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

pub async fn test_state(root: PathBuf) -> anyhow::Result<AppState> {
    let asset_dir = root.join("assets");
    tokio::fs::create_dir_all(&asset_dir).await?;
    let db = db::Db::open_local(&root.join("vulse.db")).await?;
    let conn = db.connect().await?;
    migrate::migrate(&conn).await?;
    Ok(AppState {
        db,
        password_work: std::sync::Arc::new(tokio::sync::Semaphore::new(4)),
        asset_dir,
        web_dist: root.join("missing-dist"),
    })
}

#[cfg(test)]
mod tests;
