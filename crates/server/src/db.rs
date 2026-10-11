use std::path::Path;
use std::sync::Arc;

use anyhow::Context;

#[derive(Debug, Clone)]
pub enum SqlValue {
    Null,
    Integer(i64),
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
            },
            DbInner::Remote(database) => Conn {
                inner: ConnInner::Remote(database.connect()?),
            },
        };
        conn.execute("PRAGMA foreign_keys = ON", &[]).await?;
        Ok(conn)
    }
}

impl Conn {
    pub async fn with_app_tx<T, F>(&self, f: F) -> Result<T, crate::error::AppError>
    where
        F: AsyncFnOnce(&Conn) -> Result<T, crate::error::AppError>,
    {
        self.execute("BEGIN IMMEDIATE", &[]).await?;
        match f(self).await {
            Ok(value) => match self.execute("COMMIT", &[]).await {
                Ok(_) => Ok(value),
                Err(error) => {
                    let _ = self.execute("ROLLBACK", &[]).await;
                    Err(error.into())
                }
            },
            Err(error) => {
                let _ = self.execute("ROLLBACK", &[]).await;
                Err(error)
            }
        }
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
        match f(self).await {
            Ok(value) => match self.execute("COMMIT", &[]).await {
                Ok(_) => Ok(value),
                Err(err) => {
                    let _ = self.execute("ROLLBACK", &[]).await;
                    Err(err)
                }
            },
            Err(err) => {
                let _ = self.execute("ROLLBACK", &[]).await;
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
        SqlValue::Text(value) => turso::Value::Text(value),
    }
}

fn remote_value(value: SqlValue) -> turso_serverless::Value {
    match value {
        SqlValue::Null => turso_serverless::Value::Null,
        SqlValue::Integer(value) => turso_serverless::Value::Integer(value),
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
        turso::Value::Real(_) => anyhow::bail!("unexpected real column"),
        turso::Value::Blob(_) => anyhow::bail!("unexpected blob column"),
    }
}

fn from_remote(value: turso_serverless::Value) -> anyhow::Result<SqlValue> {
    match value {
        turso_serverless::Value::Null => Ok(SqlValue::Null),
        turso_serverless::Value::Integer(value) => Ok(SqlValue::Integer(value)),
        turso_serverless::Value::Text(value) => Ok(SqlValue::Text(value)),
        turso_serverless::Value::Real(_) => anyhow::bail!("unexpected real column"),
        turso_serverless::Value::Blob(_) => anyhow::bail!("unexpected blob column"),
    }
}
