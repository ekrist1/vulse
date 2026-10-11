use std::path::PathBuf;

use crate::db::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub password_work: std::sync::Arc<tokio::sync::Semaphore>,
    pub asset_dir: PathBuf,
    pub web_dist: PathBuf,
}
