//! Change notifications for open admin views, as Server-Sent Events.
//!
//! Writes record the collections they change on their connection; the ids
//! are published when the transaction commits, so a view that reloads on
//! an event sees the new data. A one-second poll of shared database versions
//! catches writes committed by other processes and missed local notifications.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::sync::LazyLock;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use tokio::sync::broadcast;
use vulse_model::{CollectionView, lookup_references, reference_collections};

use crate::auth::AuthUser;
use crate::content::workspace_collection;
use crate::db::{Conn, Db};
use crate::error::AppError;
use crate::state::AppState;
use crate::store;

/// Ids of collections whose records or fields changed.
static CHANGES: LazyLock<broadcast::Sender<String>> = LazyLock::new(|| broadcast::channel(1024).0);

pub(crate) fn publish(collection_ids: impl IntoIterator<Item = String>) {
    for id in collection_ids {
        // No open views is not an error.
        let _ = CHANGES.send(id);
    }
}

/// The collection and those it shows data from: collections its formulas
/// read and collections its reference fields point to.
async fn watched(conn: &Conn, collection: &CollectionView) -> anyhow::Result<BTreeSet<String>> {
    let mut names: BTreeSet<String> = lookup_references(&collection.blueprint)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names.extend(
        reference_collections(&collection.blueprint)
            .into_iter()
            .map(String::from),
    );
    let mut ids = BTreeSet::from([collection.id.clone()]);
    for name in names {
        if let Some(other) = store::find_collection_by_name(conn, &name).await? {
            ids.insert(other.id);
        }
    }
    Ok(ids)
}

struct Watch {
    changes: broadcast::Receiver<String>,
    db: Db,
    id: String,
    ids: BTreeSet<String>,
    versions: BTreeMap<String, i64>,
    poll: tokio::time::Interval,
}

async fn versions(conn: &Conn, ids: &BTreeSet<String>) -> anyhow::Result<BTreeMap<String, i64>> {
    let params: Vec<_> = ids.iter().cloned().map(crate::db::SqlValue::Text).collect();
    let slots = (1..=params.len())
        .map(|n| format!("?{n}"))
        .collect::<Vec<_>>()
        .join(",");
    conn.query(
        &format!(
            "SELECT collection_id,version FROM collection_versions WHERE collection_id IN ({slots})"
        ),
        &params,
    )
    .await?
    .iter()
    .map(|r| Ok((crate::db::text_at(r, 0)?, crate::db::int_at(r, 1)?)))
    .collect()
}

impl Watch {
    async fn remote_change(&mut self) -> anyhow::Result<bool> {
        let conn = self.db.connect().await?;
        conn.with_app_read_tx(async |conn| {
            let current = versions(conn, &self.ids).await?;
            let changed = current != self.versions;
            if changed {
                if let Some(collection) = store::find_collection(conn, &self.id).await? {
                    self.ids = watched(conn, &collection).await?;
                }
                self.versions = versions(conn, &self.ids).await?;
            }
            Ok(changed)
        })
        .await
        .map_err(Into::into)
    }

    /// Waits for a change that affects the collection's view.
    async fn next(&mut self) -> Option<String> {
        loop {
            let event = tokio::select! {
                event = self.changes.recv() => event,
                _ = self.poll.tick() => {
                    match self.remote_change().await {
                        Ok(true) => return Some(self.id.clone()),
                        Ok(false) => {},
                        Err(error) => tracing::warn!(%error, "checking shared collection versions failed"),
                    }
                    continue;
                }
            };
            match event {
                Ok(changed) => {
                    if changed == self.id {
                        // Its fields may now read other collections.
                        if let Ok(conn) = self.db.connect().await
                            && let Ok(Some(collection)) =
                                store::find_collection(&conn, &self.id).await
                            && let Ok(ids) = watched(&conn, &collection).await
                        {
                            self.ids = ids;
                        }
                    }
                    if self.ids.contains(&changed) {
                        return Some(changed);
                    }
                }
                // Missed notifications: the view reloads to be safe.
                Err(broadcast::error::RecvError::Lagged(_)) => return Some(String::new()),
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

/// `GET /collections/{id}/events`: a `change` event whenever the
/// collection's records, or data its view shows, change.
pub async fn collection_events(
    State(state): State<AppState>,
    AuthUser(principal): AuthUser,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let conn = state.db.connect().await?;
    let collection = workspace_collection(&conn, &principal, &id).await?;
    let ids = watched(&conn, &collection).await?;
    let versions = versions(&conn, &ids).await?;
    let mut poll = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(1),
        Duration::from_secs(1),
    );
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let watch = Watch {
        changes: CHANGES.subscribe(),
        ids,
        versions,
        poll,
        db: state.db.clone(),
        id: collection.id,
    };
    let stream = futures_util::stream::unfold(watch, |mut watch| async move {
        let changed = watch.next().await?;
        Some((Ok(Event::default().event("change").data(changed)), watch))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))))
}
