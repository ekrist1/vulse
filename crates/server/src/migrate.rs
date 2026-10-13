use crate::db::Conn;
use crate::db::{SqlValue, text_at};
use crate::timeutil::now_rfc3339;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../migrations/001_init.sql")),
    (2, include_str!("../migrations/002_views.sql")),
    (3, include_str!("../migrations/003_actions.sql")),
    (4, include_str!("../migrations/004_history.sql")),
    (5, include_str!("../migrations/005_links.sql")),
    (6, include_str!("../migrations/006_workspace.sql")),
    (7, include_str!("../migrations/007_lookups.sql")),
    (8, include_str!("../migrations/008_cell_index.sql")),
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
                if *version == 6 {
                    merge_collection_namespaces(conn).await?;
                    // Reference names now resolve across the workspace.
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

/// Collection names were unique per user and are now unique per workspace.
/// The oldest collection keeps a contested name; later ones get the creator's
/// handle appended, and reference fields in that creator's other collections
/// follow the rename.
async fn merge_collection_namespaces(conn: &Conn) -> anyhow::Result<()> {
    use std::collections::{HashMap, HashSet};
    use vulse_model::{Blueprint, check_collection_name};

    let rows = conn
        .query(
            "SELECT c.id, c.owner_id, c.name, c.blueprint, u.handle
             FROM collections c JOIN users u ON u.id = c.owner_id
             ORDER BY c.created_at, c.id",
            &[],
        )
        .await?;
    let mut taken = HashSet::new();
    let mut renames: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut collections = Vec::new();
    for row in &rows {
        let id = text_at(row, 0)?;
        let owner = text_at(row, 1)?;
        let name = text_at(row, 2)?;
        let blueprint: Blueprint = serde_json::from_str(&text_at(row, 3)?)?;
        let handle = text_at(row, 4)?;
        let mut unique = name.clone();
        if taken.contains(&unique) {
            let base: String = format!("{name}-{handle}").chars().take(60).collect();
            let base = base.trim_end_matches('-').to_string();
            unique = base.clone();
            let mut n = 2;
            while taken.contains(&unique) || check_collection_name(&unique).is_err() {
                unique = format!("{base}-{n}");
                n += 1;
            }
            tracing::warn!(from = %name, to = %unique, "renamed collection to keep workspace names unique");
            renames
                .entry(owner.clone())
                .or_default()
                .insert(name.clone(), unique.clone());
        }
        taken.insert(unique.clone());
        collections.push((id, owner, name, unique, blueprint));
    }
    for (id, owner, name, unique, mut blueprint) in collections {
        let mut changed = name != unique;
        if let Some(map) = renames.get(&owner) {
            for field in &mut blueprint.fields {
                if let Some(target) = field.collection.as_ref().and_then(|t| map.get(t)) {
                    field.collection = Some(target.clone());
                    changed = true;
                }
            }
        }
        if changed {
            conn.execute(
                "UPDATE collections SET name = ?1, blueprint = ?2 WHERE id = ?3",
                &[
                    SqlValue::Text(unique),
                    SqlValue::Text(serde_json::to_string(&blueprint)?),
                    SqlValue::Text(id),
                ],
            )
            .await?;
        }
    }
    conn.execute(
        "CREATE UNIQUE INDEX collections_name ON collections(name)",
        &[],
    )
    .await?;
    Ok(())
}
