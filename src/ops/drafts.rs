//! Draft storage and real-time synchronization hub (AG-61, UI-47).
//!
//! A draft (project, kind, name) tracks the candidate manifest being edited, its latest
//! Verdict, who touched it last, and an optimistic concurrency version.
//!
//! Two storage arms:
//! - `Db(PgPool)`: durable PostgreSQL table backed by migration `0006_drafts.sql`.
//! - `Memory`: in-memory fallback when running without a database.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use tokio::sync::{broadcast, RwLock};
use utoipa::ToSchema;

use crate::ops::verdict::Verdict;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub project: String,
    pub kind: String,
    pub name: String,
    /// The workspace the draft belongs to, or none (CC-76): a draft of one workspace is not
    /// a draft of another, nor of the main project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub manifest: Value,
    pub verdict: Option<Verdict>,
    pub touched_by: String,
    pub touched_kind: String,
    pub version: i64,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
}

/// One line of a draft list (AG-61, API/01 §Drafts): what a caller needs to pick one, and never
/// the manifest or the verdict's trace. A project with 44 drafts answered 2.3 MB of manifests and
/// traces, which is more than one model call can carry, so the assistant answered nothing at all
/// while they existed (T-2248). A caller that needs the manifest reads it with `jc_draft_get`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DraftLine {
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub touched_by: String,
    pub touched_kind: String,
    pub version: i64,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
    /// The draft's own check, as a line: whether it passed and how much it found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<VerdictLine>,
}

/// A verdict as a line: the answer and the size of the report behind it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VerdictLine {
    pub ok: bool,
    pub findings: usize,
    #[schema(value_type = String, format = DateTime)]
    pub checked_at: DateTime<Utc>,
}

impl From<&Draft> for DraftLine {
    fn from(draft: &Draft) -> Self {
        Self {
            kind: draft.kind.clone(),
            name: draft.name.clone(),
            workspace: draft.workspace.clone(),
            touched_by: draft.touched_by.clone(),
            touched_kind: draft.touched_kind.clone(),
            version: draft.version,
            updated_at: draft.updated_at,
            verdict: draft.verdict.as_ref().map(|verdict| VerdictLine {
                ok: verdict.ok,
                findings: verdict.findings.len(),
                checked_at: verdict.checked_at,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftEvent {
    pub project: String,
    pub kind: String,
    pub name: String,
    pub version: i64,
    pub touched_by: String,
    pub touched_kind: String,
    pub event: String,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
    /// The draft's context space, so a stream filters what it forwards by the reader's binding
    /// (PF-59, T-1455). In-process only: never sent.
    #[serde(skip)]
    pub space: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("draft conflict: current version is {current}")]
    Conflict { current: i64 },
    #[error("literal secret in field '{0}' is forbidden; use secretRef instead (MF-24)")]
    Secret(String),
    #[error("database error: {0}")]
    Db(String),
    #[error("draft '{kind}/{name}' not found in project '{project}'")]
    NotFound {
        project: String,
        kind: String,
        name: String,
    },
}

#[derive(Clone, Default)]
pub struct DraftHub {
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<DraftEvent>>>>,
}

impl DraftHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn broadcast(&self, event: &DraftEvent) {
        let mut map = self.channels.write().await;
        let sender = map.entry(event.project.clone()).or_insert_with(|| {
            let (tx, _) = broadcast::channel(128);
            tx
        });
        let _ = sender.send(event.clone());
    }

    pub async fn subscribe(&self, project: &str) -> broadcast::Receiver<DraftEvent> {
        let mut map = self.channels.write().await;
        let sender = map.entry(project.to_string()).or_insert_with(|| {
            let (tx, _) = broadcast::channel(128);
            tx
        });
        sender.subscribe()
    }
}

enum DraftStoreInner {
    Db(sqlx::PgPool),
    Memory(RwLock<HashMap<(String, String, String, String), Draft>>),
}

#[derive(Clone)]
pub struct DraftStore {
    inner: Arc<DraftStoreInner>,
    hub: Option<DraftHub>,
}

/// A draft's key: its workspace (empty for none), project, kind and name.
fn key(workspace: &str, project: &str, kind: &str, name: &str) -> (String, String, String, String) {
    (
        workspace.to_owned(),
        project.to_owned(),
        kind.to_owned(),
        name.to_owned(),
    )
}

/// The store the state carries: one per process, memory-backed without a database.
pub fn draft_store(state: &AppState) -> DraftStore {
    state.drafts.clone()
}

impl DraftStore {
    pub fn new(db: Option<sqlx::PgPool>) -> Self {
        let inner = match db {
            Some(pool) => DraftStoreInner::Db(pool),
            None => DraftStoreInner::Memory(RwLock::new(HashMap::new())),
        };
        Self {
            inner: Arc::new(inner),
            hub: None,
        }
    }

    pub fn with_hub(mut self, hub: DraftHub) -> Self {
        self.hub = Some(hub);
        self
    }

    pub async fn get(
        &self,
        project: &str,
        kind: &str,
        name: &str,
    ) -> Result<Option<Draft>, DraftError> {
        self.get_in(None, project, kind, name).await
    }

    /// The draft of `kind/name` in `workspace`, or outside every workspace for `None` (CC-76).
    pub async fn get_in(
        &self,
        workspace: Option<&str>,
        project: &str,
        kind: &str,
        name: &str,
    ) -> Result<Option<Draft>, DraftError> {
        let ws = workspace.unwrap_or_default();
        match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let r = map.read().await;
                Ok(r.get(&key(ws, project, kind, name)).cloned())
            }
            DraftStoreInner::Db(pool) => {
                let row = sqlx::query(
                    "SELECT project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at \
                     FROM drafts WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $4",
                )
                .bind(project)
                .bind(kind)
                .bind(name)
                .bind(ws)
                .fetch_optional(pool)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?;

                row.map(row_to_draft).transpose()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn put(
        &self,
        project: &str,
        kind: &str,
        name: &str,
        manifest: Value,
        expected_version: Option<i64>,
        touched_by: &str,
        touched_kind: &str,
    ) -> Result<Draft, DraftError> {
        self.put_in(
            None,
            project,
            kind,
            name,
            manifest,
            expected_version,
            touched_by,
            touched_kind,
        )
        .await
    }

    /// [`DraftStore::put`] into `workspace` (CC-76).
    #[allow(clippy::too_many_arguments)]
    pub async fn put_in(
        &self,
        workspace: Option<&str>,
        project: &str,
        kind: &str,
        name: &str,
        manifest: Value,
        expected_version: Option<i64>,
        touched_by: &str,
        touched_kind: &str,
    ) -> Result<Draft, DraftError> {
        let ws = workspace.unwrap_or_default();
        if let Some(secret_field) = crate::api::mutate::find_literal_secret(&manifest) {
            return Err(DraftError::Secret(secret_field));
        }

        let _ = self.sweep(Duration::from_secs(24 * 3600)).await;

        let key = key(ws, project, kind, name);
        let draft = match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let mut w = map.write().await;
                let (new_version, verdict) = if let Some(existing) = w.get(&key) {
                    if let Some(exp) = expected_version {
                        if exp != existing.version {
                            return Err(DraftError::Conflict {
                                current: existing.version,
                            });
                        }
                    }
                    (existing.version + 1, existing.verdict.clone())
                } else {
                    if let Some(exp) = expected_version {
                        if exp != 0 {
                            return Err(DraftError::Conflict { current: 0 });
                        }
                    }
                    (1, None)
                };

                let draft = Draft {
                    project: project.to_string(),
                    kind: kind.to_string(),
                    name: name.to_string(),
                    workspace: workspace.map(str::to_owned),
                    manifest,
                    verdict,
                    touched_by: touched_by.to_string(),
                    touched_kind: touched_kind.to_string(),
                    version: new_version,
                    updated_at: Utc::now(),
                };
                w.insert(key, draft.clone());
                draft
            }
            DraftStoreInner::Db(pool) => {
                let mut tx = pool
                    .begin()
                    .await
                    .map_err(|e| DraftError::Db(e.to_string()))?;
                let existing = sqlx::query(
                    "SELECT version FROM drafts WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $4 FOR UPDATE",
                )
                .bind(project)
                .bind(kind)
                .bind(name)
                .bind(ws)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?;

                let row = match existing {
                    Some(r) => {
                        let current: i64 = r.get("version");
                        if let Some(exp) = expected_version {
                            if exp != current {
                                return Err(DraftError::Conflict { current });
                            }
                        }
                        sqlx::query(
                            "UPDATE drafts SET manifest = $4, touched_by = $5, touched_kind = $6, \
                             version = version + 1, updated_at = now() \
                             WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $7 \
                             RETURNING project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at",
                        )
                        .bind(project)
                        .bind(kind)
                        .bind(name)
                        .bind(&manifest)
                        .bind(touched_by)
                        .bind(touched_kind)
                        .bind(ws)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| DraftError::Db(e.to_string()))?
                    }
                    None => {
                        if let Some(exp) = expected_version {
                            if exp != 0 {
                                return Err(DraftError::Conflict { current: 0 });
                            }
                        }
                        sqlx::query(
                            "INSERT INTO drafts (project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at) \
                             VALUES ($1, $2, $3, $7, $4, NULL, $5, $6, 1, now()) \
                             RETURNING project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at",
                        )
                        .bind(project)
                        .bind(kind)
                        .bind(name)
                        .bind(&manifest)
                        .bind(touched_by)
                        .bind(touched_kind)
                        .bind(ws)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(|e| DraftError::Db(e.to_string()))?
                    }
                };

                tx.commit()
                    .await
                    .map_err(|e| DraftError::Db(e.to_string()))?;
                row_to_draft(row)?
            }
        };

        if let Some(hub) = &self.hub {
            hub.broadcast(&DraftEvent {
                project: draft.project.clone(),
                kind: draft.kind.clone(),
                name: draft.name.clone(),
                version: draft.version,
                touched_by: draft.touched_by.clone(),
                touched_kind: draft.touched_kind.clone(),
                event: "put".to_string(),
                updated_at: draft.updated_at,
                space: crate::permissions::space_ref(&draft.manifest),
            })
            .await;
        }

        Ok(draft)
    }

    pub async fn set_verdict(
        &self,
        project: &str,
        kind: &str,
        name: &str,
        verdict: Verdict,
    ) -> Result<Draft, DraftError> {
        self.set_verdict_in(None, project, kind, name, verdict)
            .await
    }

    /// [`DraftStore::set_verdict`] on the draft of `workspace` (CC-76).
    pub async fn set_verdict_in(
        &self,
        workspace: Option<&str>,
        project: &str,
        kind: &str,
        name: &str,
        verdict: Verdict,
    ) -> Result<Draft, DraftError> {
        let ws = workspace.unwrap_or_default();
        let key = key(ws, project, kind, name);
        let draft = match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let mut w = map.write().await;
                let draft = w.get_mut(&key).ok_or_else(|| DraftError::NotFound {
                    project: project.to_string(),
                    kind: kind.to_string(),
                    name: name.to_string(),
                })?;
                draft.verdict = Some(verdict);
                draft.updated_at = Utc::now();
                draft.clone()
            }
            DraftStoreInner::Db(pool) => {
                let verdict_json =
                    serde_json::to_value(&verdict).map_err(|e| DraftError::Db(e.to_string()))?;
                let row = sqlx::query(
                    "UPDATE drafts SET verdict = $4, updated_at = now() \
                     WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $5 \
                     RETURNING project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at",
                )
                .bind(project)
                .bind(kind)
                .bind(name)
                .bind(&verdict_json)
                .bind(ws)
                .fetch_optional(pool)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?
                .ok_or_else(|| DraftError::NotFound {
                    project: project.to_string(),
                    kind: kind.to_string(),
                    name: name.to_string(),
                })?;

                row_to_draft(row)?
            }
        };

        if let Some(hub) = &self.hub {
            hub.broadcast(&DraftEvent {
                project: draft.project.clone(),
                kind: draft.kind.clone(),
                name: draft.name.clone(),
                version: draft.version,
                touched_by: draft.touched_by.clone(),
                touched_kind: draft.touched_kind.clone(),
                event: "verdict".to_string(),
                updated_at: draft.updated_at,
                space: crate::permissions::space_ref(&draft.manifest),
            })
            .await;
        }

        Ok(draft)
    }

    pub async fn list(&self, project: &str) -> Result<Vec<Draft>, DraftError> {
        self.list_in(None, project).await
    }

    /// The drafts of `project` in `workspace`, or outside every workspace for `None` (CC-76).
    pub async fn list_in(
        &self,
        workspace: Option<&str>,
        project: &str,
    ) -> Result<Vec<Draft>, DraftError> {
        let ws = workspace.unwrap_or_default();
        match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let r = map.read().await;
                let mut drafts: Vec<Draft> = r
                    .iter()
                    .filter(|((key_ws, ..), d)| d.project == project && key_ws == ws)
                    .map(|(_, d)| d)
                    .cloned()
                    .collect();
                drafts.sort_by_key(|d| std::cmp::Reverse(d.updated_at));
                Ok(drafts)
            }
            DraftStoreInner::Db(pool) => {
                let rows = sqlx::query(
                    "SELECT project, kind, name, workspace, manifest, verdict, touched_by, touched_kind, version, updated_at \
                     FROM drafts WHERE project = $1 AND workspace = $2 ORDER BY updated_at DESC",
                )
                .bind(project)
                .bind(ws)
                .fetch_all(pool)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?;

                rows.into_iter().map(row_to_draft).collect()
            }
        }
    }

    pub async fn drop(&self, project: &str, kind: &str, name: &str) -> Result<bool, DraftError> {
        self.drop_in(None, project, kind, name).await
    }

    /// [`DraftStore::drop`] of the draft of `workspace` (CC-76).
    pub async fn drop_in(
        &self,
        workspace: Option<&str>,
        project: &str,
        kind: &str,
        name: &str,
    ) -> Result<bool, DraftError> {
        let ws = workspace.unwrap_or_default();
        let key = key(ws, project, kind, name);
        let (dropped, dropped_event) = match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let mut w = map.write().await;
                if let Some(removed) = w.remove(&key) {
                    (
                        true,
                        Some(DraftEvent {
                            space: crate::permissions::space_ref(&removed.manifest),
                            project: removed.project,
                            kind: removed.kind,
                            name: removed.name,
                            version: removed.version,
                            touched_by: removed.touched_by,
                            touched_kind: removed.touched_kind,
                            event: "drop".to_string(),
                            updated_at: Utc::now(),
                        }),
                    )
                } else {
                    (false, None)
                }
            }
            DraftStoreInner::Db(pool) => {
                let row = sqlx::query(
                    "DELETE FROM drafts WHERE project = $1 AND kind = $2 AND name = $3 AND workspace = $4 \
                     RETURNING project, kind, name, version, touched_by, touched_kind, updated_at, \
                     manifest",
                )
                .bind(project)
                .bind(kind)
                .bind(name)
                .bind(ws)
                .fetch_optional(pool)
                .await
                .map_err(|e| DraftError::Db(e.to_string()))?;

                if let Some(r) = row {
                    let version: i64 = r.get("version");
                    let touched_by: String = r.get("touched_by");
                    let touched_kind: String = r.get("touched_kind");
                    let odt: time::OffsetDateTime = r.get("updated_at");
                    let manifest: Value = r.get("manifest");
                    (
                        true,
                        Some(DraftEvent {
                            space: crate::permissions::space_ref(&manifest),
                            project: project.to_string(),
                            kind: kind.to_string(),
                            name: name.to_string(),
                            version,
                            touched_by,
                            touched_kind,
                            event: "drop".to_string(),
                            updated_at: odt_to_chrono(odt),
                        }),
                    )
                } else {
                    (false, None)
                }
            }
        };

        if let (Some(hub), Some(ev)) = (&self.hub, dropped_event) {
            hub.broadcast(&ev).await;
        }

        Ok(dropped)
    }

    pub async fn sweep(&self, idle: Duration) -> Result<(), DraftError> {
        let cutoff = Utc::now() - idle;
        match &*self.inner {
            DraftStoreInner::Memory(map) => {
                let mut w = map.write().await;
                w.retain(|_, d| d.updated_at >= cutoff);
                Ok(())
            }
            DraftStoreInner::Db(pool) => {
                let cutoff_odt = chrono_to_odt(cutoff);
                sqlx::query("DELETE FROM drafts WHERE updated_at < $1")
                    .bind(cutoff_odt)
                    .execute(pool)
                    .await
                    .map(|_| ())
                    .map_err(|e| DraftError::Db(e.to_string()))
            }
        }
    }
}

fn row_to_draft(row: sqlx::postgres::PgRow) -> Result<Draft, DraftError> {
    let project: String = row.get("project");
    let kind: String = row.get("kind");
    let name: String = row.get("name");
    let workspace: String = row.get("workspace");
    let manifest: Value = row.get("manifest");
    let verdict_val: Option<Value> = row.get("verdict");
    let verdict = verdict_val.and_then(|v| serde_json::from_value(v).ok());
    let touched_by: String = row.get("touched_by");
    let touched_kind: String = row.get("touched_kind");
    let version: i64 = row.get("version");
    let updated_at_odt: time::OffsetDateTime = row.get("updated_at");

    Ok(Draft {
        project,
        kind,
        name,
        workspace: (!workspace.is_empty()).then_some(workspace),
        manifest,
        verdict,
        touched_by,
        touched_kind,
        version,
        updated_at: odt_to_chrono(updated_at_odt),
    })
}

pub(crate) fn odt_to_chrono(odt: time::OffsetDateTime) -> DateTime<Utc> {
    let secs = odt.unix_timestamp();
    let nsecs = odt.nanosecond();
    DateTime::from_timestamp(secs, nsecs).unwrap_or_else(Utc::now)
}

pub(crate) fn chrono_to_odt(dt: DateTime<Utc>) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(dt.timestamp())
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn memory_store_lifecycle() {
        let store = DraftStore::new(None);
        let manifest = json!({ "kind": "DataSource", "metadata": { "name": "feed-1" } });

        // 1. Initial put creates version 1
        let d1 = store
            .put(
                "helsinki",
                "DataSource",
                "feed-1",
                manifest.clone(),
                None,
                "steward",
                "person",
            )
            .await
            .unwrap();
        assert_eq!(d1.version, 1);
        assert_eq!(d1.touched_by, "steward");
        assert_eq!(d1.touched_kind, "person");
        assert!(d1.verdict.is_none());

        // 2. Put with matching expected_version bumps version
        let d2 = store
            .put(
                "helsinki",
                "DataSource",
                "feed-1",
                manifest.clone(),
                Some(1),
                "assistant",
                "assistant",
            )
            .await
            .unwrap();
        assert_eq!(d2.version, 2);
        assert_eq!(d2.touched_by, "assistant");

        // 3. Put with conflicting expected_version fails
        let conflict = store
            .put(
                "helsinki",
                "DataSource",
                "feed-1",
                manifest.clone(),
                Some(1),
                "steward",
                "person",
            )
            .await
            .unwrap_err();
        match conflict {
            DraftError::Conflict { current } => assert_eq!(current, 2),
            other => panic!("expected conflict, got {other:?}"),
        }

        // 4. Literal secret is refused
        let secret_manifest =
            json!({ "kind": "DataSource", "spec": { "http": { "password": "plain" } } });
        let secret_err = store
            .put(
                "helsinki",
                "DataSource",
                "feed-sec",
                secret_manifest,
                None,
                "steward",
                "person",
            )
            .await
            .unwrap_err();
        assert!(matches!(secret_err, DraftError::Secret(_)));

        // 5. Set verdict attaches verdict without bumping version
        let verdict = Verdict::green(&manifest, None);
        let d_v = store
            .set_verdict("helsinki", "DataSource", "feed-1", verdict.clone())
            .await
            .unwrap();
        assert_eq!(d_v.version, 2);
        assert!(d_v.verdict.is_some());

        // 6. List and get
        let got = store
            .get("helsinki", "DataSource", "feed-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.version, 2);
        assert_eq!(got.verdict, Some(verdict));

        let items = store.list("helsinki").await.unwrap();
        assert_eq!(items.len(), 1);

        // 7. Drop
        let dropped = store
            .drop("helsinki", "DataSource", "feed-1")
            .await
            .unwrap();
        assert!(dropped);
        assert!(store
            .get("helsinki", "DataSource", "feed-1")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn draft_hub_broadcasts_events() {
        let hub = DraftHub::new();
        let mut sub = hub.subscribe("helsinki").await;
        let store = DraftStore::new(None).with_hub(hub);

        let manifest = json!({ "kind": "Endpoint", "metadata": { "name": "ep-1" } });
        let _ = store
            .put(
                "helsinki",
                "Endpoint",
                "ep-1",
                manifest.clone(),
                None,
                "steward",
                "person",
            )
            .await
            .unwrap();

        let ev = sub.recv().await.unwrap();
        assert_eq!(ev.project, "helsinki");
        assert_eq!(ev.kind, "Endpoint");
        assert_eq!(ev.name, "ep-1");
        assert_eq!(ev.event, "put");
        assert_eq!(ev.version, 1);
    }
}
