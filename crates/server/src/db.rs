use std::path::Path;
use std::sync::Arc;

use anyhow::Context;

#[derive(Debug, Clone)]
pub enum SqlValue {
    Null,
    Integer(i64),
    /// Aggregates such as SUM and AVG over stored numbers.
    Real(f64),
    Text(String),
}

pub type SqlRow = Vec<SqlValue>;

#[derive(Clone)]
pub struct Db {
    inner: Arc<DbInner>,
}

enum DbInner {
    Local(turso::Database),
    Remote(turso_serverless::Database),
}

pub struct Conn {
    inner: ConnInner,
    /// Collections changed in the open transaction, published on commit.
    changes: std::sync::Mutex<Option<std::collections::BTreeSet<String>>>,
    pub(crate) lookup_cache: crate::lookups::Cache,
    /// Recorded in the history of records this connection changes.
    source: std::sync::OnceLock<&'static str>,
}

enum ConnInner {
    Local(turso::Connection),
    Remote(turso_serverless::Connection),
}

impl Db {
    pub async fn open_local(path: &Path) -> anyhow::Result<Self> {
        let path = path.to_str().context("database path must be utf-8")?;
        let database = turso::Builder::new_local(path).build().await?;
        Ok(Self {
            inner: Arc::new(DbInner::Local(database)),
        })
    }

    pub async fn open_remote(url: &str, token: &str) -> anyhow::Result<Self> {
        let database = turso_serverless::Builder::new_remote(url)
            .with_auth_token(token)
            .build()
            .await?;
        Ok(Self {
            inner: Arc::new(DbInner::Remote(database)),
        })
    }

    pub async fn connect(&self) -> anyhow::Result<Conn> {
        let conn = match &*self.inner {
            DbInner::Local(database) => Conn {
                inner: ConnInner::Local(database.connect()?),
                changes: Default::default(),
                lookup_cache: Default::default(),
                source: Default::default(),
            },
            DbInner::Remote(database) => Conn {
                inner: ConnInner::Remote(database.connect()?),
                changes: Default::default(),
                lookup_cache: Default::default(),
                source: Default::default(),
            },
        };
        if let ConnInner::Local(local) = &conn.inner {
            // Wait asynchronously for overlapping writers, including standalone
            // session and asset writes. The wait is bounded for a stuck writer.
            local.busy_timeout(std::time::Duration::from_secs(5))?;
        }
        conn.execute("PRAGMA foreign_keys = ON", &[]).await?;
        Ok(conn)
    }
}

impl Conn {
    /// Notes that a collection's records or fields changed. Open views are
    /// told once the transaction commits, or right away outside one.
    /// Marks this connection's record changes as coming from `source`, such as
    /// `mcp`. The first call wins.
    pub fn set_source(&self, source: &'static str) {
        let _ = self.source.set(source);
    }

    pub fn source(&self) -> Option<&'static str> {
        self.source.get().copied()
    }

    pub fn changed(&self, collection_id: &str) {
        self.lookup_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(collection_id);
        let mut changes = self.changes.lock().unwrap_or_else(|e| e.into_inner());
        match changes.as_mut() {
            Some(pending) => {
                pending.insert(collection_id.to_string());
            }
            None => {
                crate::lookups::invalidate(collection_id);
                crate::events::publish([collection_id.to_string()]);
            }
        }
    }
    fn begin_changes(&self) {
        self.lookup_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        *self.changes.lock().unwrap_or_else(|e| e.into_inner()) = Some(Default::default());
    }
    pub(crate) fn in_transaction(&self) -> bool {
        self.changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
    /// Ends the transaction's change list, publishing it if committed.
    fn end_changes(&self, committed: bool) {
        let pending = self
            .changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if committed && let Some(pending) = pending {
            for id in &pending {
                crate::lookups::invalidate(id);
            }
            crate::events::publish(pending);
        }
        self.lookup_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
    pub async fn with_app_tx<T, F>(&self, f: F) -> Result<T, crate::error::AppError>
    where
        F: AsyncFnOnce(&Conn) -> Result<T, crate::error::AppError>,
    {
        self.with_app_transaction(true, f).await
    }

    /// A consistent read snapshot without reserving the database's writer.
    pub async fn with_app_read_tx<T, F>(&self, f: F) -> Result<T, crate::error::AppError>
    where
        F: AsyncFnOnce(&Conn) -> Result<T, crate::error::AppError>,
    {
        self.with_app_transaction(false, f).await
    }

    pub async fn with_app_transaction<T, F>(
        &self,
        write: bool,
        f: F,
    ) -> Result<T, crate::error::AppError>
    where
        F: AsyncFnOnce(&Conn) -> Result<T, crate::error::AppError>,
    {
        self.execute(if write { "BEGIN IMMEDIATE" } else { "BEGIN" }, &[])
            .await?;
        self.begin_changes();
        let result = match f(self).await {
            // Collections reading the changed ones get current values in the
            // same transaction.
            Ok(value) => crate::records::refresh_readers(self, &self.pending_changes())
                .await
                .map(|()| value)
                .map_err(Into::into),
            Err(error) => Err(error),
        };
        match result {
            Ok(value) => match self.execute("COMMIT", &[]).await {
                Ok(_) => {
                    self.end_changes(true);
                    Ok(value)
                }
                Err(error) => {
                    let _ = self.execute("ROLLBACK", &[]).await;
                    self.end_changes(false);
                    Err(error.into())
                }
            },
            Err(error) => {
                let _ = self.execute("ROLLBACK", &[]).await;
                self.end_changes(false);
                Err(error)
            }
        }
    }
    /// Collections changed so far in the open transaction.
    fn pending_changes(&self) -> std::collections::BTreeSet<String> {
        self.changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default()
    }
    pub async fn execute(&self, sql: &str, params: &[SqlValue]) -> anyhow::Result<u64> {
        let result = match &self.inner {
            ConnInner::Local(conn) => {
                let values = params.iter().cloned().map(local_value).collect::<Vec<_>>();
                conn.execute(sql, values).await?
            }
            ConnInner::Remote(conn) => {
                let values = params.iter().cloned().map(remote_value).collect::<Vec<_>>();
                conn.execute(sql, values).await?
            }
        };
        Ok(result)
    }

    pub async fn query(&self, sql: &str, params: &[SqlValue]) -> anyhow::Result<Vec<SqlRow>> {
        match &self.inner {
            ConnInner::Local(conn) => {
                let values = params.iter().cloned().map(local_value).collect::<Vec<_>>();
                let mut rows = conn.query(sql, values).await?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().await? {
                    out.push(read_local_row(&row)?);
                }
                Ok(out)
            }
            ConnInner::Remote(conn) => {
                let values = params.iter().cloned().map(remote_value).collect::<Vec<_>>();
                let mut rows = conn.query(sql, values).await?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().await? {
                    out.push(read_remote_row(&row)?);
                }
                Ok(out)
            }
        }
    }

    pub async fn execute_batch(&self, sql: &str) -> anyhow::Result<()> {
        match &self.inner {
            ConnInner::Local(conn) => conn.execute_batch(sql).await?,
            ConnInner::Remote(conn) => conn.execute_batch(sql).await?,
        }
        Ok(())
    }

    pub async fn with_tx<T, F>(&self, f: F) -> anyhow::Result<T>
    where
        F: AsyncFnOnce(&Conn) -> anyhow::Result<T>,
    {
        self.execute("BEGIN IMMEDIATE", &[]).await?;
        self.begin_changes();
        match f(self).await {
            Ok(value) => match self.execute("COMMIT", &[]).await {
                Ok(_) => {
                    self.end_changes(true);
                    Ok(value)
                }
                Err(err) => {
                    let _ = self.execute("ROLLBACK", &[]).await;
                    self.end_changes(false);
                    Err(err)
                }
            },
            Err(err) => {
                let _ = self.execute("ROLLBACK", &[]).await;
                self.end_changes(false);
                Err(err)
            }
        }
    }
}

pub fn text_at(row: &SqlRow, index: usize) -> anyhow::Result<String> {
    match row.get(index) {
        Some(SqlValue::Text(value)) => Ok(value.clone()),
        Some(other) => anyhow::bail!("column {index} is {other:?}, expected text"),
        None => anyhow::bail!("missing column {index}"),
    }
}

pub fn int_at(row: &SqlRow, index: usize) -> anyhow::Result<i64> {
    match row.get(index) {
        Some(SqlValue::Integer(value)) => Ok(*value),
        Some(other) => anyhow::bail!("column {index} is {other:?}, expected integer"),
        None => anyhow::bail!("missing column {index}"),
    }
}

/// A numeric column as a float, or `None` for NULL.
pub fn opt_number_at(row: &SqlRow, index: usize) -> anyhow::Result<Option<f64>> {
    match row.get(index) {
        Some(SqlValue::Null) | None => Ok(None),
        Some(SqlValue::Integer(value)) => Ok(Some(*value as f64)),
        Some(SqlValue::Real(value)) => Ok(Some(*value)),
        Some(other) => anyhow::bail!("column {index} is {other:?}, expected a number"),
    }
}

pub fn opt_text_at(row: &SqlRow, index: usize) -> anyhow::Result<Option<String>> {
    match row.get(index) {
        Some(SqlValue::Null) | None => Ok(None),
        Some(SqlValue::Text(value)) => Ok(Some(value.clone())),
        Some(other) => anyhow::bail!("column {index} is {other:?}, expected text or null"),
    }
}

fn local_value(value: SqlValue) -> turso::Value {
    match value {
        SqlValue::Null => turso::Value::Null,
        SqlValue::Integer(value) => turso::Value::Integer(value),
        SqlValue::Real(value) => turso::Value::Real(value),
        SqlValue::Text(value) => turso::Value::Text(value),
    }
}

fn remote_value(value: SqlValue) -> turso_serverless::Value {
    match value {
        SqlValue::Null => turso_serverless::Value::Null,
        SqlValue::Integer(value) => turso_serverless::Value::Integer(value),
        SqlValue::Real(value) => turso_serverless::Value::Real(value),
        SqlValue::Text(value) => turso_serverless::Value::Text(value),
    }
}

fn read_local_row(row: &turso::Row) -> anyhow::Result<SqlRow> {
    let mut out = Vec::with_capacity(row.column_count());
    for index in 0..row.column_count() {
        out.push(from_local(row.get_value(index)?)?);
    }
    Ok(out)
}

fn read_remote_row(row: &turso_serverless::Row) -> anyhow::Result<SqlRow> {
    let mut out = Vec::with_capacity(row.column_count());
    for index in 0..row.column_count() {
        out.push(from_remote(row.get_value(index)?)?);
    }
    Ok(out)
}

fn from_local(value: turso::Value) -> anyhow::Result<SqlValue> {
    match value {
        turso::Value::Null => Ok(SqlValue::Null),
        turso::Value::Integer(value) => Ok(SqlValue::Integer(value)),
        turso::Value::Text(value) => Ok(SqlValue::Text(value)),
        turso::Value::Real(value) => Ok(SqlValue::Real(value)),
        turso::Value::Blob(_) => anyhow::bail!("unexpected blob column"),
    }
}

fn from_remote(value: turso_serverless::Value) -> anyhow::Result<SqlValue> {
    match value {
        turso_serverless::Value::Null => Ok(SqlValue::Null),
        turso_serverless::Value::Integer(value) => Ok(SqlValue::Integer(value)),
        turso_serverless::Value::Text(value) => Ok(SqlValue::Text(value)),
        turso_serverless::Value::Real(value) => Ok(SqlValue::Real(value)),
        turso_serverless::Value::Blob(_) => anyhow::bail!("unexpected blob column"),
    }
}
