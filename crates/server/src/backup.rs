//! Backups while the server runs: one zip with a consistent copy of the
//! database (`VACUUM INTO`) and the uploaded files. Archives are written under
//! a temporary name and renamed when complete, so a listed backup is whole.
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use vulse_model::{BackupOverview, BackupView};

use crate::auth::AdminSession;
use crate::db::Db;
use crate::error::AppError;
use crate::state::AppState;

/// One backup at a time, whether scheduled or asked for by `vulse backup`.
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const PREFIX: &str = "vulse-backup-";
const SUFFIX: &str = ".zip";

/// A finished backup.
pub struct Backup {
    pub path: PathBuf,
    pub bytes: u64,
    pub assets: usize,
}

/// Writes `vulse-backup-<UTC time>.zip` to `out_dir` and returns its path.
/// The archive holds `vulse.db`, `storage/…` with the uploaded files, and
/// `backup.json` describing them.
pub async fn create(db: &Db, asset_dir: &Path, out_dir: &Path) -> anyhow::Result<Backup> {
    let _running = RUNNING.lock().await;
    tokio::fs::create_dir_all(out_dir)
        .await
        .with_context(|| format!("could not create {}", out_dir.display()))?;
    let out_dir = tokio::fs::canonicalize(out_dir).await?;
    // Only one backup runs at a time, so leftovers are from an interrupted one.
    remove_partials(&out_dir).await?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let snapshot = out_dir.join(format!(".{PREFIX}{stamp}-{}.db", uuid::Uuid::new_v4()));
    let snapshot_sql = snapshot
        .to_str()
        .filter(|path| !path.contains('\''))
        .context("the backup directory path must be UTF-8 without quote characters")?;
    let conn = db.connect().await?;
    let result = async {
        conn.execute(&format!("VACUUM INTO '{snapshot_sql}'"), &[])
            .await
            .context("could not copy the database")?;
        let final_path = out_dir.join(format!("{PREFIX}{stamp}{SUFFIX}"));
        let partial = out_dir.join(format!(".{PREFIX}{stamp}{SUFFIX}.partial"));
        let (snapshot_file, asset_dir) = (snapshot.clone(), asset_dir.to_path_buf());
        let partial_file = partial.clone();
        let assets = tokio::task::spawn_blocking(move || {
            write_archive(&partial_file, &snapshot_file, &asset_dir, &stamp)
        })
        .await??;
        tokio::fs::rename(&partial, &final_path).await?;
        let bytes = tokio::fs::metadata(&final_path).await?.len();
        Ok(Backup {
            path: final_path,
            bytes,
            assets,
        })
    }
    .await;
    // The snapshot may leave a WAL beside it; neither is needed once archived.
    for path in [snapshot.clone(), wal_of(&snapshot)] {
        let _ = tokio::fs::remove_file(path).await;
    }
    if result.is_err() {
        let _ = remove_partials(&out_dir).await;
    }
    result
}

fn wal_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

fn write_archive(
    out: &Path,
    snapshot: &Path,
    asset_dir: &Path,
    created_at: &str,
) -> anyhow::Result<usize> {
    let mut zip = zip::ZipWriter::new(BufWriter::new(File::create(out)?));
    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);
    // Uploads are mostly images, video and PDFs that are already compressed.
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);
    zip.start_file("vulse.db", deflated)?;
    std::io::copy(&mut BufReader::new(File::open(snapshot)?), &mut zip)?;
    let mut files = Vec::new();
    if asset_dir.is_dir() {
        collect_files(asset_dir, asset_dir, &mut files)?;
    }
    files.sort();
    for relative in &files {
        let name = format!(
            "storage/{}",
            relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        );
        // A file deleted since it was listed is skipped; it is gone from the data too.
        let Ok(file) = File::open(asset_dir.join(relative)) else {
            continue;
        };
        zip.start_file(name, stored)?;
        std::io::copy(&mut BufReader::new(file), &mut zip)?;
    }
    zip.start_file("backup.json", deflated)?;
    let manifest = serde_json::json!({
        "format": 1,
        "vulse_version": env!("CARGO_PKG_VERSION"),
        "created_at": created_at,
        "database": "vulse.db",
        "assets": files.len(),
        "restore": "Stop Vulse, put vulse.db at VULSE_DB_PATH (removing any vulse.db-wal beside it) and storage/ at ASSET_DIR, then start Vulse.",
    });
    zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
    zip.finish()?.flush()?;
    Ok(files.len())
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            collect_files(root, &entry.path(), out)?;
        } else if kind.is_file() {
            out.push(entry.path().strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

/// Backups in `dir`, oldest first. Names sort by time.
pub async fn list(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(e) => return Err(e.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(PREFIX) && name.ends_with(SUFFIX) {
            found.push(entry.path());
        }
    }
    found.sort();
    Ok(found)
}

/// Deletes all but the newest `keep` backups in `dir` and returns how many it removed.
pub async fn prune(dir: &Path, keep: usize) -> anyhow::Result<usize> {
    let backups = list(dir).await?;
    let excess = backups.len().saturating_sub(keep);
    for path in &backups[..excess] {
        tokio::fs::remove_file(path).await?;
    }
    Ok(excess)
}

/// Removes archives and snapshots left by an interrupted backup.
async fn remove_partials(dir: &Path) -> anyhow::Result<()> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!(".{PREFIX}")) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
    Ok(())
}

/// Backups made from the admin, in the scheduled backup directory when set.
#[derive(Debug)]
pub struct Settings {
    pub dir: PathBuf,
    pub schedule: Option<Schedule>,
}

fn settings(state: &AppState) -> Result<&Settings, AppError> {
    state.backup.as_deref().ok_or_else(|| {
        AppError::bad("backups are not available for a Turso database; Turso keeps its own")
    })
}

async fn view(path: &Path) -> anyhow::Result<BackupView> {
    let name = path
        .file_name()
        .context("backup without a name")?
        .to_string_lossy()
        .into_owned();
    let stamp = &name[PREFIX.len()..name.len() - SUFFIX.len()];
    let created_at = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ")
        .map(|t| t.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default();
    Ok(BackupView {
        bytes: tokio::fs::metadata(path).await?.len(),
        name,
        created_at,
    })
}

/// A backup in `dir` by its listed name, never a path outside it.
fn named(dir: &Path, name: &str) -> Result<PathBuf, AppError> {
    let valid = name.starts_with(PREFIX)
        && name.ends_with(SUFFIX)
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
        && !name.contains("..");
    if !valid {
        return Err(AppError::NotFound);
    }
    Ok(dir.join(name))
}

pub async fn list_backups(
    State(state): State<AppState>,
    AdminSession(_admin): AdminSession,
) -> Result<Json<BackupOverview>, AppError> {
    let settings = settings(&state)?;
    let mut backups = Vec::new();
    for path in list(&settings.dir).await?.iter().rev() {
        // A backup deleted while listing is skipped.
        if let Ok(backup) = view(path).await {
            backups.push(backup);
        }
    }
    Ok(Json(BackupOverview {
        dir: settings.dir.display().to_string(),
        every_hours: settings.schedule.as_ref().map(|s| s.every.as_secs() / 3600),
        keep: settings.schedule.as_ref().map(|s| s.keep),
        backups,
    }))
}

pub async fn create_backup(
    State(state): State<AppState>,
    AdminSession(admin): AdminSession,
) -> Result<(StatusCode, Json<BackupView>), AppError> {
    let settings = settings(&state)?;
    let made = create(&state.db, &state.asset_dir, &settings.dir).await?;
    tracing::info!(path = %made.path.display(), bytes = made.bytes, user = %admin.user_id, "backup written");
    Ok((StatusCode::CREATED, Json(view(&made.path).await?)))
}

pub async fn download_backup(
    State(state): State<AppState>,
    AdminSession(admin): AdminSession,
    AxumPath(name): AxumPath<String>,
) -> Result<Response, AppError> {
    let path = named(&settings(&state)?.dir, &name)?;
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(AppError::NotFound),
        Err(e) => return Err(anyhow::anyhow!(e).into()),
    };
    let bytes = file.metadata().await.map_err(anyhow::Error::from)?.len();
    tracing::info!(%name, user = %admin.user_id, "backup downloaded");
    let mut response = Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file)));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(bytes));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

pub async fn delete_backup(
    State(state): State<AppState>,
    AdminSession(admin): AdminSession,
    AxumPath(name): AxumPath<String>,
) -> Result<StatusCode, AppError> {
    let path = named(&settings(&state)?.dir, &name)?;
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {
            tracing::info!(%name, user = %admin.user_id, "backup deleted");
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AppError::NotFound),
        Err(e) => Err(anyhow::anyhow!(e).into()),
    }
}

/// Scheduled backups: `VULSE_BACKUP_DIR` turns them on.
#[derive(Debug, Clone)]
pub struct Schedule {
    pub dir: PathBuf,
    pub every: Duration,
    pub keep: usize,
}

/// Backs up on `schedule`, starting one interval after startup, and keeps the
/// newest `keep` archives. Failures are logged and retried at the next interval.
pub fn spawn_scheduled(db: Db, asset_dir: PathBuf, schedule: Schedule) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(schedule.every);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            match create(&db, &asset_dir, &schedule.dir).await {
                Ok(backup) => {
                    tracing::info!(
                        path = %backup.path.display(),
                        bytes = backup.bytes,
                        assets = backup.assets,
                        "backup written"
                    );
                    match prune(&schedule.dir, schedule.keep).await {
                        Ok(0) => {}
                        Ok(removed) => tracing::info!(removed, "old backups removed"),
                        Err(e) => tracing::warn!("could not remove old backups: {e:#}"),
                    }
                }
                Err(e) => tracing::error!("backup failed: {e:#}"),
            }
        }
    });
}

/// The control socket a running server listens on next to its database, so
/// `vulse backup` can ask it for a backup while it holds the database lock.
pub fn socket_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_owned();
    name.push("-backup.sock");
    PathBuf::from(name)
}

/// A backup request from `vulse backup`, as one JSON line.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub dir: PathBuf,
    pub keep: Option<usize>,
}

/// The server's answer, as one JSON line.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Reply {
    pub path: Option<PathBuf>,
    pub bytes: u64,
    pub assets: usize,
    pub removed: usize,
    pub error: Option<String>,
}

/// Listens on the control socket. Only the server's own user (and root) can
/// connect.
#[cfg(unix)]
pub fn spawn_socket(db: Db, asset_dir: PathBuf, db_path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = socket_path(db_path);
    // A socket left by a stopped server; a running one would hold the database lock.
    let _ = std::fs::remove_file(&path);
    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                "`vulse backup` cannot reach this server, so it can only back up while Vulse is stopped: {e}"
            );
            return;
        }
    };
    // Connecting needs write permission on the socket, which the default
    // umask already limits to the owner; 0600 also removes read access.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let (db, asset_dir) = (db.clone(), asset_dir.clone());
            tokio::spawn(async move {
                if let Err(e) = answer(stream, &db, &asset_dir).await {
                    tracing::warn!("backup request failed: {e:#}");
                }
            });
        }
    });
}

#[cfg(unix)]
async fn answer(stream: tokio::net::UnixStream, db: &Db, asset_dir: &Path) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    tokio::io::BufReader::new(read.take(64 * 1024))
        .read_line(&mut line)
        .await?;
    let reply = match serde_json::from_str::<Request>(&line) {
        Ok(request) if request.dir.is_absolute() => {
            match create(db, asset_dir, &request.dir).await {
                Ok(backup) => {
                    tracing::info!(path = %backup.path.display(), bytes = backup.bytes, "backup written");
                    let removed = match request.keep {
                        Some(keep) => prune(&request.dir, keep.max(1)).await.unwrap_or(0),
                        None => 0,
                    };
                    Reply {
                        path: Some(backup.path),
                        bytes: backup.bytes,
                        assets: backup.assets,
                        removed,
                        error: None,
                    }
                }
                Err(e) => failure(format!("{e:#}")),
            }
        }
        Ok(_) => failure("the backup directory must be an absolute path".into()),
        Err(e) => failure(format!("invalid request: {e}")),
    };
    let mut text = serde_json::to_string(&reply)?;
    text.push('\n');
    write.write_all(text.as_bytes()).await?;
    Ok(())
}

fn failure(error: String) -> Reply {
    Reply {
        path: None,
        bytes: 0,
        assets: 0,
        removed: 0,
        error: Some(error),
    }
}

/// Asks the server running on `db_path` for a backup. `None` when no server answers.
#[cfg(unix)]
pub async fn request(db_path: &Path, request: &Request) -> anyhow::Result<Option<Reply>> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let path = socket_path(db_path);
    let stream = match tokio::net::UnixStream::connect(&path).await {
        Ok(stream) => stream,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None);
        }
        Err(e) => {
            return Err(e).with_context(|| format!("could not reach Vulse at {}", path.display()));
        }
    };
    let (read, mut write) = stream.into_split();
    let mut text = serde_json::to_string(request)?;
    text.push('\n');
    write.write_all(text.as_bytes()).await?;
    let mut line = String::new();
    tokio::io::BufReader::new(read).read_line(&mut line).await?;
    Ok(Some(serde_json::from_str(&line).context("unexpected reply from Vulse")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn backups_hold_a_consistent_database_and_the_uploads() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_state(dir.path().to_path_buf()).await.unwrap();
        let conn = state.db.connect().await.unwrap();
        conn.execute("INSERT INTO users(id,handle,email,password_hash,created_at) VALUES('u','ada','ada@example.com','x','2026-01-01T00:00:00Z')", &[]).await.unwrap();
        let upload = state.asset_dir.join("owner").join("asset");
        std::fs::create_dir_all(upload.parent().unwrap()).unwrap();
        std::fs::write(&upload, b"image bytes").unwrap();
        let out = dir.path().join("backups");
        // Left by an interrupted backup.
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join(".vulse-backup-old.db"), b"partial").unwrap();

        let made = create(&state.db, &state.asset_dir, &out).await.unwrap();
        assert_eq!(made.assets, 1);
        let names: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![made.path.file_name().unwrap().to_string_lossy().into_owned()]);

        let mut zip = zip::ZipArchive::new(File::open(&made.path).unwrap()).unwrap();
        let mut upload_copy = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("storage/owner/asset").unwrap(), &mut upload_copy).unwrap();
        assert_eq!(upload_copy, "image bytes");
        let manifest: serde_json::Value = serde_json::from_reader(zip.by_name("backup.json").unwrap()).unwrap();
        assert_eq!(manifest["assets"], 1);
        let restored = dir.path().join("restored.db");
        std::io::copy(&mut zip.by_name("vulse.db").unwrap(), &mut File::create(&restored).unwrap()).unwrap();
        let restored = Db::open_local(&restored).await.unwrap().connect().await.unwrap();
        let rows = restored.query("SELECT handle FROM users", &[]).await.unwrap();
        assert_eq!(crate::db::text_at(&rows[0], 0).unwrap(), "ada");
    }

    #[tokio::test]
    async fn prune_keeps_the_newest_backups() {
        let dir = tempfile::tempdir().unwrap();
        for stamp in ["20260101T000000Z", "20260102T000000Z", "20260103T000000Z"] {
            std::fs::write(dir.path().join(format!("{PREFIX}{stamp}{SUFFIX}")), b"").unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), b"").unwrap();
        assert_eq!(prune(dir.path(), 2).await.unwrap(), 1);
        let left: Vec<_> = list(dir.path()).await.unwrap().into_iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["vulse-backup-20260102T000000Z.zip", "vulse-backup-20260103T000000Z.zip"]);
        assert!(dir.path().join("notes.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_running_server_answers_backup_requests_on_its_socket() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_state(dir.path().to_path_buf()).await.unwrap();
        let db_path = dir.path().join("vulse.db");
        assert!(request(&db_path, &Request { dir: dir.path().into(), keep: None }).await.unwrap().is_none());
        spawn_socket(state.db.clone(), state.asset_dir.clone(), &db_path);
        let mode = std::fs::metadata(socket_path(&db_path)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let out = dir.path().join("backups");
        let reply = request(&db_path, &Request { dir: out.clone(), keep: Some(1) }).await.unwrap().unwrap();
        assert!(reply.error.is_none());
        assert!(reply.path.unwrap().starts_with(std::fs::canonicalize(&out).unwrap()));
        let refused = request(&db_path, &Request { dir: "relative".into(), keep: None }).await.unwrap().unwrap();
        assert!(refused.error.unwrap().contains("absolute"));
    }
}
