//! The workspace registry (CC-76, CC-81, ADR-N-024).
//!
//! A workspace is the branch `workspace/{name}` of the Organization repository, with an owner,
//! the revision it was branched from, what it covers and when it expires. The record describes
//! a branch, so it lives beside the drafts, not on the branch. Two storage arms, like drafts:
//! the `workspaces` table (migration `0015_workspaces.sql`), or memory without a database.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tokio::sync::RwLock;
use utoipa::ToSchema;

use crate::ops::drafts::{chrono_to_odt, odt_to_chrono};

/// The longest a workspace lives: two weeks, like a sandbox (CC-67, PF-19).
pub const MAX_TTL_HOURS: i64 = 14 * 24;

/// The longest workspace name: `ws-{name}-` goes in front of a project name, and the result
/// is still one DNS label (CC-78).
pub const MAX_NAME: usize = 20;

/// What a workspace covers (CC-76, API/01 §22): the whole project, one space and what it
/// holds, or a list of resources.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Scope {
    Project {},
    Space { name: String },
    Resources { items: Vec<ScopedResource> },
}

/// One resource a workspace covers.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScopedResource {
    pub kind: String,
    pub name: String,
}

impl Scope {
    /// Every name a DNS label, every kind one the platform serves, and a list of resources
    /// that lists something.
    pub fn validate(&self) -> Result<(), WorkspaceError> {
        let invalid = |why: String| Err(WorkspaceError::Invalid(why));
        match self {
            Self::Project {} => Ok(()),
            Self::Space { name } if !crate::resource::is_dns1123(name) => {
                invalid(format!("'{name}' is not a space name"))
            }
            Self::Space { .. } => Ok(()),
            Self::Resources { items } if items.is_empty() => {
                invalid("a resources scope lists at least one resource".into())
            }
            Self::Resources { items } => {
                for item in items {
                    if crate::resource::by_kind(&item.kind).is_none() {
                        return invalid(format!(
                            "'{}' is not a kind this platform serves",
                            item.kind
                        ));
                    }
                    if !crate::resource::is_dns1123(&item.name) {
                        return invalid(format!("'{}' is not a resource name", item.name));
                    }
                }
                Ok(())
            }
        }
    }

    /// Whether `kind/name`, whose space is `space`, is inside the scope.
    pub fn covers(&self, kind: &str, name: &str, space: Option<&str>) -> bool {
        match self {
            Self::Project {} => true,
            Self::Space { name: scoped } => {
                (kind == "ContextSpace" && name == scoped) || space == Some(scoped.as_str())
            }
            Self::Resources { items } => items
                .iter()
                .any(|item| item.kind == kind && item.name == name),
        }
    }
}

/// Where a workspace's preview is (CC-78).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PreviewState {
    None,
    Starting,
    Running,
    Stopped,
    Error,
}

impl PreviewState {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Error => "error",
        }
    }

    fn parse(text: &str) -> Self {
        match text {
            "starting" => Self::Starting,
            "running" => Self::Running,
            "stopped" => Self::Stopped,
            "error" => Self::Error,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub project: String,
    pub owner: String,
    pub base_revision: String,
    pub scope: Scope,
    pub preview_state: PreviewState,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<Utc>,
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<Utc>,
}

impl Workspace {
    /// The branch the workspace is (CC-76).
    pub fn branch(&self) -> String {
        branch_of(&self.name)
    }

    /// Whether the workspace has outlived its TTL at `now` (CC-81).
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now
    }
}

/// The branch of the workspace `name`.
pub fn branch_of(name: &str) -> String {
    format!("workspace/{name}")
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("{0}")]
    Invalid(String),
    #[error("a workspace named '{0}' already exists")]
    Conflict(String),
    #[error("no workspace named '{0}'")]
    NotFound(String),
    #[error("database error: {0}")]
    Db(String),
}

/// What opening a workspace asks for.
#[derive(Debug, Clone)]
pub struct Opening<'a> {
    pub name: &'a str,
    pub title: Option<&'a str>,
    pub project: &'a str,
    pub owner: &'a str,
    pub base_revision: &'a str,
    pub scope: Scope,
    pub ttl_hours: i64,
}

enum Inner {
    Db(sqlx::PgPool),
    Memory(RwLock<BTreeMap<String, Workspace>>),
}

#[derive(Clone)]
pub struct WorkspaceStore {
    inner: Arc<Inner>,
}

fn row_to_workspace(row: sqlx::postgres::PgRow) -> Result<Workspace, WorkspaceError> {
    let scope: serde_json::Value = row.get("scope");
    Ok(Workspace {
        name: row.get("name"),
        title: row.get("title"),
        project: row.get("project"),
        owner: row.get("owner"),
        base_revision: row.get("base_revision"),
        scope: serde_json::from_value(scope).map_err(|e| WorkspaceError::Db(e.to_string()))?,
        preview_state: PreviewState::parse(row.get::<String, _>("preview_state").as_str()),
        created_at: odt_to_chrono(row.get("created_at")),
        expires_at: odt_to_chrono(row.get("expires_at")),
    })
}

fn db(err: sqlx::Error) -> WorkspaceError {
    WorkspaceError::Db(err.to_string())
}

impl WorkspaceStore {
    pub fn new(pool: Option<sqlx::PgPool>) -> Self {
        let inner = match pool {
            Some(pool) => Inner::Db(pool),
            None => Inner::Memory(RwLock::new(BTreeMap::new())),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Records a new workspace; its name is unique across the organization, because the
    /// branch and the preview prefix are (CC-76, CC-78).
    pub async fn create(&self, opening: Opening<'_>) -> Result<Workspace, WorkspaceError> {
        if opening.name.len() > MAX_NAME || !crate::resource::is_dns1123(opening.name) {
            return Err(WorkspaceError::Invalid(format!(
                "a workspace name is a DNS label of at most {MAX_NAME} characters"
            )));
        }
        if !(1..=MAX_TTL_HOURS).contains(&opening.ttl_hours) {
            return Err(WorkspaceError::Invalid(format!(
                "a workspace lives between 1 and {MAX_TTL_HOURS} hours"
            )));
        }
        opening.scope.validate()?;
        let now = Utc::now();
        let workspace = Workspace {
            name: opening.name.to_owned(),
            title: opening.title.map(str::to_owned),
            project: opening.project.to_owned(),
            owner: opening.owner.to_owned(),
            base_revision: opening.base_revision.to_owned(),
            scope: opening.scope,
            preview_state: PreviewState::None,
            created_at: now,
            expires_at: now + Duration::hours(opening.ttl_hours),
        };
        match &*self.inner {
            Inner::Memory(map) => {
                let mut map = map.write().await;
                if map.contains_key(&workspace.name) {
                    return Err(WorkspaceError::Conflict(workspace.name));
                }
                map.insert(workspace.name.clone(), workspace.clone());
                Ok(workspace)
            }
            Inner::Db(pool) => {
                let scope = serde_json::to_value(&workspace.scope)
                    .map_err(|e| WorkspaceError::Db(e.to_string()))?;
                let row = sqlx::query(
                    "INSERT INTO workspaces (name, title, project, owner, base_revision, scope, preview_state, created_at, expires_at) VALUES ($1, $8, $2, $3, $4, $5, 'none', $6, $7) \
                     ON CONFLICT (name) DO NOTHING RETURNING name, title, project, owner, base_revision, scope, preview_state, created_at, expires_at",
                )
                .bind(&workspace.name)
                .bind(&workspace.project)
                .bind(&workspace.owner)
                .bind(&workspace.base_revision)
                .bind(scope)
                .bind(chrono_to_odt(workspace.created_at))
                .bind(chrono_to_odt(workspace.expires_at))
                .bind(&workspace.title)
                .fetch_optional(pool)
                .await
                .map_err(db)?;
                match row {
                    Some(row) => row_to_workspace(row),
                    None => Err(WorkspaceError::Conflict(workspace.name)),
                }
            }
        }
    }

    pub async fn get(&self, name: &str) -> Result<Option<Workspace>, WorkspaceError> {
        match &*self.inner {
            Inner::Memory(map) => Ok(map.read().await.get(name).cloned()),
            Inner::Db(pool) => {
                sqlx::query("SELECT name, title, project, owner, base_revision, scope, preview_state, created_at, expires_at FROM workspaces WHERE name = $1")
                    .bind(name)
                    .fetch_optional(pool)
                    .await
                    .map_err(db)?
                    .map(row_to_workspace)
                    .transpose()
            }
        }
    }

    /// The live workspace `name`: absent and expired answer the same `NotFound`, so an expired
    /// workspace takes nothing more (CC-81).
    pub async fn live(&self, name: &str) -> Result<Workspace, WorkspaceError> {
        match self.get(name).await? {
            Some(workspace) if !workspace.expired(Utc::now()) => Ok(workspace),
            _ => Err(WorkspaceError::NotFound(name.to_owned())),
        }
    }

    /// Every workspace of `project`, oldest first.
    pub async fn list(&self, project: &str) -> Result<Vec<Workspace>, WorkspaceError> {
        let mut found = match &*self.inner {
            Inner::Memory(map) => map
                .read()
                .await
                .values()
                .filter(|w| w.project == project)
                .cloned()
                .collect::<Vec<_>>(),
            Inner::Db(pool) => sqlx::query("SELECT name, title, project, owner, base_revision, scope, preview_state, created_at, expires_at FROM workspaces WHERE project = $1")
            .bind(project)
            .fetch_all(pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(row_to_workspace)
            .collect::<Result<Vec<_>, _>>()?,
        };
        found.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.name.cmp(&b.name)));
        Ok(found)
    }

    pub async fn set_preview_state(
        &self,
        name: &str,
        state: PreviewState,
    ) -> Result<(), WorkspaceError> {
        match &*self.inner {
            Inner::Memory(map) => match map.write().await.get_mut(name) {
                Some(workspace) => {
                    workspace.preview_state = state;
                    Ok(())
                }
                None => Err(WorkspaceError::NotFound(name.to_owned())),
            },
            Inner::Db(pool) => {
                let done = sqlx::query("UPDATE workspaces SET preview_state = $2 WHERE name = $1")
                    .bind(name)
                    .bind(state.as_str())
                    .execute(pool)
                    .await
                    .map_err(db)?;
                if done.rows_affected() == 0 {
                    return Err(WorkspaceError::NotFound(name.to_owned()));
                }
                Ok(())
            }
        }
    }

    /// Removes the record; the branch and the preview are the caller's to remove (CC-81).
    pub async fn delete(&self, name: &str) -> Result<Workspace, WorkspaceError> {
        match &*self.inner {
            Inner::Memory(map) => map
                .write()
                .await
                .remove(name)
                .ok_or_else(|| WorkspaceError::NotFound(name.to_owned())),
            Inner::Db(pool) => sqlx::query("DELETE FROM workspaces WHERE name = $1 RETURNING name, title, project, owner, base_revision, scope, preview_state, created_at, expires_at")
            .bind(name)
            .fetch_optional(pool)
            .await
            .map_err(db)?
            .map(row_to_workspace)
            .transpose()?
            .ok_or_else(|| WorkspaceError::NotFound(name.to_owned())),
        }
    }

    /// The workspaces whose TTL has passed at `now`, for the reaper (CC-81).
    pub async fn expired(&self, now: DateTime<Utc>) -> Result<Vec<Workspace>, WorkspaceError> {
        match &*self.inner {
            Inner::Memory(map) => Ok(map
                .read()
                .await
                .values()
                .filter(|w| w.expired(now))
                .cloned()
                .collect()),
            Inner::Db(pool) => sqlx::query("SELECT name, title, project, owner, base_revision, scope, preview_state, created_at, expires_at FROM workspaces WHERE expires_at <= $1 ORDER BY name")
            .bind(chrono_to_odt(now))
            .fetch_all(pool)
            .await
            .map_err(db)?
            .into_iter()
            .map(row_to_workspace)
            .collect(),
        }
    }
}

/// What the Portal reads inside a workspace: `main` as the mirror holds it, with every manifest
/// the workspace's branch changed laid over it and every one it removed taken out (CC-76, CC-77).
///
/// Two trees are compared by blob id, so only the files the workspace touched are read. A
/// workspace whose branch has no commit yet reads as `main`. Not persisted: it is computed for
/// the request that asks.
pub async fn mirror_of(
    state: &crate::state::AppState,
    name: &str,
    project: &str,
) -> Result<crate::store::Mirror, crate::error::ApiError> {
    use crate::error::ApiError;
    let workspace = state
        .workspaces
        .live(name)
        .await
        .map_err(|err| ApiError::NotFound(err.to_string()))?;
    if workspace.project != project {
        return Err(ApiError::NotFound(format!(
            "no workspace named '{name}' in project '{project}'"
        )));
    }
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let main = gitea.default_branch().await?;
    let view = state.mirror.snapshot();
    let branch = workspace.branch();
    let theirs = match gitea.list_tree_blobs(&branch).await {
        Ok(tree) => tree,
        Err(crate::git::GitError::NotFound) => return Ok(view),
        Err(err) => return Err(err.into()),
    };
    let ours: BTreeMap<String, String> = gitea.list_tree_blobs(&main).await?.into_iter().collect();
    let is_manifest = |path: &str| path.ends_with(".yaml") || path.ends_with(".yml");
    let mut kept = std::collections::BTreeSet::new();
    for (path, sha) in theirs {
        kept.insert(path.clone());
        if !is_manifest(&path) || ours.get(&path) == Some(&sha) {
            continue;
        }
        if let Some(file) = gitea.get_file(&path, &branch).await? {
            if let Some(envelope) = crate::store::envelope_of(&file.content) {
                view.upsert(envelope);
            }
        }
    }
    for path in ours
        .keys()
        .filter(|path| is_manifest(path) && !kept.contains(*path))
    {
        if let Some(file) = gitea.get_file(path, &main).await? {
            if let Some(envelope) = crate::store::envelope_of(&file.content) {
                view.remove(&envelope.key());
            }
        }
    }
    Ok(view)
}
