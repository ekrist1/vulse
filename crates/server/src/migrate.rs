use crate::db::Conn;
use crate::db::SqlValue;
use crate::timeutil::now_rfc3339;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/001_init.sql")),
    (2, include_str!("../migrations/002_views.sql")),
    (3, include_str!("../migrations/003_actions.sql")),
    (4, include_str!("../migrations/004_history.sql")),
    (5, include_str!("../migrations/005_links.sql")),
];

pub async fn migrate(conn: &Conn) -> anyhow::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        )",
        &[],
    )
    .await?;
    for (version, sql) in MIGRATIONS {
        let existing = conn
            .query(
                "SELECT version FROM schema_migrations WHERE version = ?1",
                &[SqlValue::Integer(*version)],
            )
            .await?;
        if existing.is_empty() {
            conn.with_tx(async |conn| {
                conn.execute_batch(sql).await?;
                if *version == 2 {
                    crate::records::rebuild_all_cells(conn).await?;
                }
                if *version == 5 {
                    crate::relationships::rebuild(conn).await?;
                }
                conn.execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                    &[SqlValue::Integer(*version), SqlValue::Text(now_rfc3339())],
                )
                .await?;
                Ok(())
            })
            .await?;
        }
    }
    Ok(())
}
