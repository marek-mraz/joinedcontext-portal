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

    /// Moves the revision the workspace compares against, after main was merged into it (CC-80).
    pub async fn set_base_revision(
        &self,
        name: &str,
        revision: &str,
    ) -> Result<(), WorkspaceError> {
        match &*self.inner {
            Inner::Memory(map) => match map.write().await.get_mut(name) {
                Some(workspace) => {
                    workspace.base_revision = revision.to_owned();
                    Ok(())
                }
                None => Err(WorkspaceError::NotFound(name.to_owned())),
            },
            Inner::Db(pool) => {
                let done = sqlx::query("UPDATE workspaces SET base_revision = $2 WHERE name = $1")
                    .bind(name)
                    .bind(revision)
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
/// the workspace's branch changed since its base laid over it and every one it removed taken
/// out (CC-76, CC-77). Compared with the base and not with main, so a file main changed after
/// the workspace opened reads as main has it, not as the branch's older copy.
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
    let base = workspace.base_revision.clone();
    let view = state.mirror.snapshot();
    let branch = workspace.branch();
    let theirs = match gitea.list_tree_blobs(&branch).await {
        Ok(tree) => tree,
        Err(crate::git::GitError::NotFound) => return Ok(view),
        Err(err) => return Err(err.into()),
    };
    let ours: BTreeMap<String, String> = gitea.list_tree_blobs(&base).await?.into_iter().collect();
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
        if let Some(file) = gitea.get_file(path, &base).await? {
            if let Some(envelope) = crate::store::envelope_of(&file.content) {
                view.remove(&envelope.key());
            }
        }
    }
    Ok(view)
}

// ---------------------------------------------------------------------------------------------
// The actions of API/01 §22, one implementation behind the REST routes and the operations
// (T-1236, T-1237). Opening, writing and bringing back need `propose` in the project; reading
// needs `read`; only the owner writes into a workspace, updates it, brings it back or discards
// it. An agent may open, write and compare, and never brings one back (AG-82).
// ---------------------------------------------------------------------------------------------

use crate::api::changes::ChangeFile;
use crate::auth::session::Identity;
use crate::change::{
    Change, ChangeMeta, ChangePhase, ChangeStatus, Lane, Operation as Op, PlanSummary,
};
use crate::error::ApiError;
use crate::git::{GitError, GiteaClient};
use crate::ops::{Caller, OpError, Via};
use crate::plan::{FieldConflict, Side};
use crate::state::AppState;
use serde_json::{json, Value};

/// A workspace's time to live when the opening names none, in days (API/01 §22).
pub const DEFAULT_TTL_DAYS: i64 = 7;
/// The longest title a workspace carries.
pub const MAX_TITLE: usize = 120;

/// What opening a workspace asks for (API/01 §22).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OpenRequest {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    /// The whole project when absent.
    #[serde(default)]
    pub scope: Option<Scope>,
    /// Seven when absent, at most fourteen.
    #[serde(default)]
    pub ttl_days: Option<i64>,
}

/// The side a person kept for one conflicting field of one file (CC-80).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Resolution {
    /// The file's path in the repository, as the comparison lists it.
    pub path: String,
    /// The field's path, as the conflict lists it; empty for the whole file.
    #[serde(default)]
    pub field: String,
    pub keep: Side,
}

/// What updating a workspace from main asks for: the answer to every conflict (CC-80).
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UpdateRequest {
    #[serde(default)]
    pub resolutions: Vec<Resolution>,
}

/// A workspace as the API answers it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    #[serde(flatten)]
    pub workspace: Workspace,
    pub branch: String,
    /// How many files it changes; read only when one workspace is asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<usize>,
}

impl From<Workspace> for WorkspaceView {
    fn from(workspace: Workspace) -> Self {
        Self {
            branch: workspace.branch(),
            workspace,
            changes: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceList {
    pub items: Vec<WorkspaceView>,
}

/// One file both the workspace and main changed since the workspace's base (CC-80). `fields`
/// lists what a person has to choose; empty means the two sides merge on their own, which
/// updating from main does.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct FileConflict {
    pub path: String,
    pub fields: Vec<FieldConflict>,
}

/// What a workspace changes against its base, and where main changed the same files (API/01 §22).
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct Comparison {
    pub files: Vec<ChangeFile>,
    pub conflicts: Vec<FileConflict>,
}

/// What updating from main did (CC-80).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReport {
    /// Files main changed that the workspace had not touched, now as main has them.
    pub taken: Vec<String>,
    /// Files both changed, merged field by field and checked again.
    pub merged: Vec<String>,
    pub base_revision: String,
    pub comparison: Comparison,
}

impl From<WorkspaceError> for ApiError {
    fn from(err: WorkspaceError) -> Self {
        match err {
            WorkspaceError::Invalid(msg) => ApiError::BadRequest(msg),
            WorkspaceError::Conflict(_) => ApiError::Conflict(err.to_string()),
            WorkspaceError::NotFound(_) => ApiError::NotFound(err.to_string()),
            WorkspaceError::Db(msg) => ApiError::Internal(msg),
        }
    }
}

/// Who owns what `identity` opens: the address a commit is signed with, as drafts are.
pub fn owner_of(identity: &Identity) -> String {
    identity
        .email
        .clone()
        .unwrap_or_else(|| identity.username.clone())
}

/// Whether `identity` owns `workspace`.
pub fn owns(workspace: &Workspace, identity: &Identity) -> bool {
    workspace.owner == identity.username || identity.email.as_deref() == Some(&workspace.owner)
}

fn forge(state: &AppState) -> Result<&GiteaClient, ApiError> {
    state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))
}

/// A project the caller reads nothing of is not there (PF-59, R20).
fn readable(state: &AppState, identity: &Identity, project: &str) -> Result<(), ApiError> {
    if crate::permissions::for_request(state, identity, project).may_read_project() {
        Ok(())
    } else {
        Err(ApiError::NotFound(format!("project '{project}' not found")))
    }
}

/// Whether `identity` may see `workspace` (PF-59, CC-79): its owner always; anyone else needs
/// `read` on what it covers — the project, the space, or every listed resource's kind. What the
/// caller may not see is not there: a 404, and absent from a list, never a 403.
pub fn may_see(
    state: &AppState,
    effective: &crate::permissions::Effective,
    workspace: &Workspace,
    identity: &Identity,
) -> bool {
    if owns(workspace, identity) {
        return true;
    }
    match &workspace.scope {
        // A binding to one space reads that space, not a workspace over the whole project.
        Scope::Project {} => {
            effective.bootstrap || effective.grants.iter().any(|grant| grant.space.is_none())
        }
        Scope::Space { name } => effective.may_read_in("ContextSpace", Some(name)),
        // Each resource as the caller could read it on main: in its own space; one the workspace
        // adds is not on main yet and needs a grant on its kind across the project.
        Scope::Resources { items } => items.iter().all(|item| {
            match state.mirror.get(&workspace.project, &item.kind, &item.name) {
                Some(envelope) => serde_json::to_value(&envelope)
                    .is_ok_and(|manifest| effective.may_read_manifest(&item.kind, &manifest)),
                None => effective.may_read_in(&item.kind, None),
            }
        }),
    }
}

/// The live workspace `name` of `project`, readable by the caller.
async fn visible(
    state: &AppState,
    identity: &Identity,
    project: &str,
    name: &str,
) -> Result<Workspace, ApiError> {
    readable(state, identity, project)?;
    let workspace = state.workspaces.live(name).await?;
    let effective = crate::permissions::for_request(state, identity, project);
    if workspace.project != project || !may_see(state, &effective, &workspace, identity) {
        return Err(ApiError::NotFound(format!(
            "no workspace named '{name}' in project '{project}'"
        )));
    }
    Ok(workspace)
}

/// [`visible`], and the caller's own: only the owner changes a workspace (API/01 §22).
async fn owned(
    state: &AppState,
    identity: &Identity,
    project: &str,
    name: &str,
    action: &str,
) -> Result<Workspace, ApiError> {
    let workspace = visible(state, identity, project, name).await?;
    if !owns(&workspace, identity) {
        return Err(ApiError::Denied(format!(
            "workspace '{name}' belongs to {}; only its owner may {action} it",
            workspace.owner
        )));
    }
    Ok(workspace)
}

/// Opens a workspace: the record, then its branch from main's head (CC-76).
pub async fn open(
    identity: &Identity,
    state: &AppState,
    project: &str,
    request: OpenRequest,
) -> Result<WorkspaceView, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    if !effective.may_propose_anything() {
        return Err(ApiError::Denied(format!(
            "Opening a workspace needs a role with propose in {project}."
        )));
    }
    let title = request
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    if let Some(title) = title {
        if title.chars().count() > MAX_TITLE || title.chars().any(char::is_control) {
            return Err(ApiError::BadRequest(format!(
                "a workspace title is one line of at most {MAX_TITLE} characters"
            )));
        }
    }
    let days = request.ttl_days.unwrap_or(DEFAULT_TTL_DAYS);
    if !(1..=MAX_TTL_HOURS / 24).contains(&days) {
        return Err(ApiError::BadRequest(format!(
            "ttlDays is between 1 and {}",
            MAX_TTL_HOURS / 24
        )));
    }
    let gitea = forge(state)?;
    let main = gitea.default_branch().await?;
    let base = gitea.branch_head(&main).await?;
    let owner = owner_of(identity);
    let workspace = state
        .workspaces
        .create(Opening {
            name: &request.name,
            title,
            project,
            owner: &owner,
            base_revision: &base,
            scope: request.scope.unwrap_or(Scope::Project {}),
            ttl_hours: days * 24,
        })
        .await?;
    // A branch of this name outlived a workspace that was reaped or discarded without it: the
    // record is new and unique, so the branch is nobody's and starts again from main.
    let branch = workspace.branch();
    let created = match gitea.create_branch(&branch, &main).await {
        Err(GitError::Conflict(_)) => match gitea.delete_branch(&branch).await {
            Ok(()) => gitea.create_branch(&branch, &main).await,
            Err(err) => Err(err),
        },
        other => other,
    };
    if let Err(err) = created {
        let _ = state.workspaces.delete(&workspace.name).await;
        return Err(err.into());
    }
    Ok(workspace.into())
}

pub async fn list(
    identity: &Identity,
    state: &AppState,
    project: &str,
) -> Result<WorkspaceList, ApiError> {
    readable(state, identity, project)?;
    let now = Utc::now();
    let effective = crate::permissions::for_request(state, identity, project);
    let items = state
        .workspaces
        .list(project)
        .await?
        .into_iter()
        .filter(|w| !w.expired(now) && may_see(state, &effective, w, identity))
        .map(WorkspaceView::from)
        .collect();
    Ok(WorkspaceList { items })
}

pub async fn get(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<WorkspaceView, ApiError> {
    let workspace = visible(state, identity, project, name).await?;
    let changes = compare_workspace(state, &workspace).await?.files.len();
    let mut view = WorkspaceView::from(workspace);
    view.changes = Some(changes);
    Ok(view)
}

pub async fn compare(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<Comparison, ApiError> {
    let workspace = visible(state, identity, project, name).await?;
    compare_workspace(state, &workspace).await
}

/// The three trees a comparison reads: the workspace's branch, its base and main now.
struct Trees {
    main: String,
    ours: BTreeMap<String, String>,
    base: BTreeMap<String, String>,
    theirs: BTreeMap<String, String>,
}

async fn trees(gitea: &GiteaClient, workspace: &Workspace) -> Result<Option<Trees>, ApiError> {
    let ours = match gitea.list_tree_blobs(&workspace.branch()).await {
        Ok(tree) => tree.into_iter().collect(),
        Err(GitError::NotFound) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let main = gitea.default_branch().await?;
    let base = gitea
        .list_tree_blobs(&workspace.base_revision)
        .await?
        .into_iter()
        .collect();
    let theirs = gitea.list_tree_blobs(&main).await?.into_iter().collect();
    Ok(Some(Trees {
        main,
        ours,
        base,
        theirs,
    }))
}

fn is_manifest(path: &str) -> bool {
    path.ends_with(".yaml") || path.ends_with(".yml")
}

/// A manifest file as a document, when `path` is one and `git_ref` has it.
async fn document(
    gitea: &GiteaClient,
    path: &str,
    git_ref: &str,
) -> Result<Option<Value>, ApiError> {
    if !is_manifest(path) {
        return Ok(None);
    }
    let Some(file) = gitea.get_file(path, git_ref).await? else {
        return Ok(None);
    };
    Ok(serde_yaml_ng::from_str::<Value>(&file.content).ok())
}

fn envelope(doc: Option<&Value>) -> Option<crate::resource::ResourceEnvelope> {
    doc.and_then(|doc| serde_json::from_value(doc.clone()).ok())
}

/// Masks the value of a field that holds a credential by its name (CC-06).
fn masked(mut conflicts: Vec<FieldConflict>) -> Vec<FieldConflict> {
    let hidden = || Value::String("[REDACTED]".into());
    for conflict in &mut conflicts {
        if crate::api::changes::is_sensitive_path(&conflict.path) {
            conflict.ours = hidden();
            conflict.theirs = hidden();
            conflict.base = hidden();
        }
    }
    conflicts
}

/// Compares a workspace with its base, file by file, and with main where main moved (CC-79, CC-80).
pub async fn compare_workspace(
    state: &AppState,
    workspace: &Workspace,
) -> Result<Comparison, ApiError> {
    let gitea = forge(state)?;
    let Some(trees) = trees(gitea, workspace).await? else {
        return Ok(Comparison::default());
    };
    let branch = workspace.branch();
    let touched: std::collections::BTreeSet<&String> = trees
        .ours
        .keys()
        .chain(trees.base.keys())
        .filter(|path| trees.ours.get(*path) != trees.base.get(*path))
        .collect();
    let mut comparison = Comparison::default();
    for path in touched {
        let ours_doc = if trees.ours.contains_key(path) {
            document(gitea, path, &branch).await?
        } else {
            None
        };
        let base_doc = if trees.base.contains_key(path) {
            document(gitea, path, &workspace.base_revision).await?
        } else {
            None
        };
        let operation = match (trees.base.contains_key(path), trees.ours.contains_key(path)) {
            (false, _) => Op::Create,
            (true, false) => Op::Delete,
            (true, true) => Op::Update,
        };
        let ours_env = envelope(ours_doc.as_ref());
        let base_env = envelope(base_doc.as_ref());
        let kind = ours_env
            .as_ref()
            .or(base_env.as_ref())
            .map(|env| env.kind.clone())
            .or_else(|| crate::api::import::native_kind(path).map(str::to_owned));
        let Some(kind) = kind else {
            continue;
        };
        let spec = ours_env
            .as_ref()
            .map(|env| env.spec.clone())
            .unwrap_or(Value::Null);
        let fields = (ours_env.is_some() || base_env.is_some()).then(|| {
            crate::api::changes::redact(
                crate::plan::diff(base_env.as_ref(), ours_env.as_ref()).fields,
            )
        });
        comparison.files.push(ChangeFile {
            path: path.clone(),
            lane: crate::change::classify(&kind, operation, &spec),
            kind,
            operation,
            fields,
        });
        let theirs = trees.theirs.get(path);
        if theirs == trees.base.get(path) || theirs == trees.ours.get(path) {
            continue;
        }
        let theirs_doc = if theirs.is_some() {
            document(gitea, path, &trees.main).await?
        } else {
            None
        };
        let fields = match (&ours_doc, &theirs_doc) {
            (Some(ours), Some(theirs)) => {
                match crate::plan::merge3(base_doc.as_ref(), ours, theirs, &|_| None) {
                    Ok(_) => Vec::new(),
                    Err(conflicts) => masked(conflicts),
                }
            }
            // Removed on one side and changed on the other, or a file that is not a manifest:
            // the whole file is the one choice.
            _ => vec![FieldConflict {
                path: String::new(),
                ours: if trees.ours.contains_key(path) {
                    json!("changed")
                } else {
                    json!("removed")
                },
                theirs: if theirs.is_some() {
                    json!("changed")
                } else {
                    json!("removed")
                },
                base: Value::Null,
            }],
        };
        comparison.conflicts.push(FileConflict {
            path: path.clone(),
            fields,
        });
    }
    Ok(comparison)
}

/// Checks a manifest again as its proposal would be checked, without proposing it (CC-80).
async fn recheck(
    identity: &Identity,
    state: &AppState,
    project: &str,
    path: &str,
    doc: &Value,
    operation: Op,
) -> Result<(), ApiError> {
    let Some(env) = envelope(Some(doc)) else {
        return Err(ApiError::BadRequest(format!("{path}: not a manifest")));
    };
    let plural = crate::resource::by_kind(&env.kind)
        .ok_or_else(|| ApiError::BadRequest(format!("{path}: kind '{}' is not served", env.kind)))?
        .plural;
    let name = env.metadata.name.clone();
    let outcome = crate::api::mutate::propose_with_identity(
        identity,
        state,
        project,
        plural,
        (operation == Op::Update).then_some(name.as_str()),
        operation,
        true,
        doc.clone(),
    )
    .await
    .map_err(|err| match err {
        ApiError::BadRequest(msg) => ApiError::BadRequest(format!("{path}: {msg}")),
        other => other,
    })?;
    match outcome {
        crate::api::mutate::ProposeOutcome::DryRun(result) if !result.valid => {
            Err(ApiError::BadRequest(format!(
                "{path}: its check found problems; fix them in the workspace"
            )))
        }
        _ => Ok(()),
    }
}

/// Merges main into the workspace: what only main changed is taken, what both changed is
/// merged field by field with the person's answers, and every merged manifest is checked
/// again before it is written (CC-80). A conflict without an answer refuses the whole update.
pub async fn update_from_main(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
    request: UpdateRequest,
) -> Result<UpdateReport, OpError> {
    let workspace = owned(state, identity, project, name, "update").await?;
    let gitea = forge(state)?;
    let main = gitea.default_branch().await.map_err(ApiError::from)?;
    let head = gitea.branch_head(&main).await.map_err(ApiError::from)?;
    let branch = workspace.branch();
    let mut taken = Vec::new();
    let mut merged = Vec::new();
    if let Some(trees) = trees(gitea, &workspace).await? {
        let pick = |path: &str, field: &str| {
            request
                .resolutions
                .iter()
                .find(|r| r.path == path && r.field == field)
                .map(|r| r.keep)
        };
        let mut uploads: Vec<(String, String)> = Vec::new();
        let mut deletes: Vec<(String, String)> = Vec::new();
        let mut unresolved = Vec::new();
        let paths: std::collections::BTreeSet<&String> = trees
            .ours
            .keys()
            .chain(trees.base.keys())
            .chain(trees.theirs.keys())
            .collect();
        for path in paths {
            let (b, o, t) = (
                trees.base.get(path),
                trees.ours.get(path),
                trees.theirs.get(path),
            );
            if t == b || o == t {
                continue;
            }
            let take_theirs = |uploads: &mut Vec<(String, String)>,
                               deletes: &mut Vec<(String, String)>,
                               content: Option<String>| {
                match content {
                    Some(content) => uploads.push((path.clone(), content)),
                    None => {
                        if let Some(sha) = o {
                            deletes.push((path.clone(), sha.clone()));
                        }
                    }
                }
            };
            let theirs_text = match t {
                Some(_) => gitea
                    .get_file(path, &trees.main)
                    .await
                    .map_err(ApiError::from)?
                    .map(|f| f.content),
                None => None,
            };
            if o == b {
                take_theirs(&mut uploads, &mut deletes, theirs_text);
                taken.push(path.clone());
                continue;
            }
            let ours_doc = document(gitea, path, &branch).await?;
            let theirs_doc = theirs_text
                .as_deref()
                .filter(|_| is_manifest(path))
                .and_then(|text| serde_yaml_ng::from_str::<Value>(text).ok());
            match (ours_doc, theirs_doc) {
                (Some(ours), Some(theirs)) => {
                    let base_doc = document(gitea, path, &workspace.base_revision).await?;
                    match crate::plan::merge3(base_doc.as_ref(), &ours, &theirs, &|field| {
                        pick(path, field)
                    }) {
                        Ok(doc) => {
                            recheck(identity, state, project, path, &doc, Op::Update).await?;
                            let text = serde_yaml_ng::to_string(&doc)
                                .map_err(|e| ApiError::Internal(e.to_string()))?;
                            uploads.push((path.clone(), text));
                            merged.push(path.clone());
                        }
                        Err(conflicts) => unresolved.push(FileConflict {
                            path: path.clone(),
                            fields: masked(conflicts),
                        }),
                    }
                }
                _ => match pick(path, "") {
                    Some(Side::Ours) => {}
                    Some(Side::Theirs) => {
                        take_theirs(&mut uploads, &mut deletes, theirs_text);
                        merged.push(path.clone());
                    }
                    None => unresolved.push(FileConflict {
                        path: path.clone(),
                        fields: vec![FieldConflict {
                            path: String::new(),
                            ours: if o.is_some() {
                                json!("changed")
                            } else {
                                json!("removed")
                            },
                            theirs: if t.is_some() {
                                json!("changed")
                            } else {
                                json!("removed")
                            },
                            base: Value::Null,
                        }],
                    }),
                },
            }
        }
        if !unresolved.is_empty() {
            return Err(OpError::Conflict(json!({
                "error": "conflict",
                "detail": "Main changed the same fields; choose ours or theirs for each, then update again (CC-80).",
                "conflicts": unresolved,
            })));
        }
        if !uploads.is_empty() || !deletes.is_empty() {
            let (author_name, author_email) =
                crate::api::mutate::author_credentials(identity, project);
            gitea
                .change_files(
                    &branch,
                    &format!("update workspace {name} from {main}"),
                    crate::git::gitea::Author {
                        name: &author_name,
                        email: &author_email,
                    },
                    &uploads,
                    &deletes,
                )
                .await
                .map_err(ApiError::from)?;
        }
    }
    state
        .workspaces
        .set_base_revision(name, &head)
        .await
        .map_err(ApiError::from)?;
    let workspace = state.workspaces.live(name).await.map_err(ApiError::from)?;
    let comparison = compare_workspace(state, &workspace).await?;
    Ok(UpdateReport {
        taken,
        merged,
        base_revision: head,
        comparison,
    })
}

/// An agent, or an MCP client a model drives, never brings a workspace back (AG-82, as AG-11
/// for a decision): asked before anything runs, so the tool is not even offered.
pub fn refuse_agent_bring_back(caller: &Caller) -> Result<(), OpError> {
    if matches!(caller.via, Via::Agent | Via::Mcp) {
        return Err(OpError::Api(ApiError::Denied(
            "an agent never brings a workspace back; it presents the comparison and a person \
             proposes it in the Portal or over the REST route (AG-82)"
                .into(),
        )));
    }
    Ok(())
}

/// Brings a workspace back as one Change: the pull request of its branch, in the lane of its
/// riskiest file (CC-79). Refused while a conflict stands, when it changes nothing, when a
/// touched manifest fails its check again (CC-80, PF-57), and always for an agent (AG-82).
pub async fn propose(
    caller: &Caller,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<Change, OpError> {
    refuse_agent_bring_back(caller)?;
    let identity = &caller.identity;
    let workspace = owned(state, identity, project, name, "bring back").await?;
    let comparison = compare_workspace(state, &workspace).await?;
    if !comparison.conflicts.is_empty() {
        return Err(OpError::Conflict(json!({
            "error": "conflict",
            "detail": "Main changed files this workspace changes; update it from main and resolve each conflict first (CC-80).",
            "conflicts": comparison.conflicts,
        })));
    }
    if comparison.files.is_empty() {
        return Err(OpError::Api(ApiError::Conflict(format!(
            "workspace '{name}' changes nothing yet"
        ))));
    }
    let gitea = forge(state)?;
    let branch = workspace.branch();
    if let Some(open) = crate::api::mutate::open_change_on(gitea, &branch, project).await? {
        return Err(OpError::Api(ApiError::Conflict(format!(
            "workspace '{name}' is already brought back as {}; approve or reject it first",
            open.name
        ))));
    }
    let effective = crate::permissions::for_request(state, identity, project);
    let mut lane = Lane::Green;
    let mut summary = PlanSummary::default();
    for file in &comparison.files {
        let git_ref = if file.operation == Op::Delete {
            workspace.base_revision.clone()
        } else {
            branch.clone()
        };
        let doc = document(gitea, &file.path, &git_ref).await?;
        let verb = if file.operation == Op::Delete {
            jc_core::kinds::Verb::Delete
        } else {
            jc_core::kinds::Verb::Propose
        };
        effective.check(&file.kind, verb, doc.as_ref())?;
        if let (Some(doc), false) = (&doc, file.operation == Op::Delete) {
            recheck(identity, state, project, &file.path, doc, file.operation).await?;
        }
        lane = crate::api::import::riskiest(lane, file.lane);
        match file.operation {
            Op::Create => summary.create += 1,
            Op::Update => summary.update += 1,
            Op::Delete => summary.delete += 1,
        }
    }
    let main = gitea.default_branch().await.map_err(ApiError::from)?;
    let title = format!(
        "workspace {name}: {}",
        workspace.title.as_deref().unwrap_or(name)
    );
    let body = format!(
        "Brought back from workspace `{name}` of {} in project `{project}`: {} files, based on {}.",
        workspace.owner,
        comparison.files.len(),
        workspace.base_revision
    );
    let pr = gitea
        .create_pull_request(&branch, &main, &title, &body)
        .await
        .map_err(ApiError::from)?;
    let status =
        ChangeStatus::new(lane, ChangePhase::PendingApproval, summary).with_merge_request(pr.url);
    Ok(Change::new(
        ChangeMeta::from_merge_request(pr.number, project),
        status,
    ))
}

/// Discards a workspace: its branch and its record go; an open bring-back closes with the
/// branch (CC-81).
pub async fn discard(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<(), ApiError> {
    let workspace = owned(state, identity, project, name, "discard").await?;
    forge(state)?.delete_branch(&workspace.branch()).await?;
    state.workspaces.delete(name).await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct NameInput {
    name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UpdateInput {
    name: String,
    #[serde(default)]
    resolutions: Vec<Resolution>,
}

fn name_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "name": { "type": "string", "description": "The workspace's name" } },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn scope_schema() -> Value {
    json!({
        "type": "object",
        "description": "{kind: project}, {kind: space, name}, or {kind: resources, items: [{kind, name}]}",
        "properties": {
            "kind": { "type": "string", "enum": ["project", "space", "resources"] },
            "name": { "type": "string" },
            "items": { "type": "array", "items": {
                "type": "object",
                "properties": { "kind": { "type": "string" }, "name": { "type": "string" } },
                "required": ["kind", "name"]
            } }
        },
        "required": ["kind"]
    })
}

fn open_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "description": "A DNS label of at most 20 characters, unique in the organization" },
            "title": { "type": "string" },
            "scope": scope_schema(),
            "ttlDays": { "type": "integer", "minimum": 1, "maximum": 14 }
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn update_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "resolutions": { "type": "array", "items": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The file, as the comparison lists it" },
                    "field": { "type": "string", "description": "The field, as the conflict lists it; empty for the whole file" },
                    "keep": { "type": "string", "enum": ["ours", "theirs"] }
                },
                "required": ["path", "keep"],
                "additionalProperties": false
            } }
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn workspace_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }, "title": { "type": "string" },
            "project": { "type": "string" }, "owner": { "type": "string" },
            "branch": { "type": "string" }, "baseRevision": { "type": "string" },
            "scope": scope_schema(), "previewState": { "type": "string" },
            "createdAt": { "type": "string" }, "expiresAt": { "type": "string" },
            "changes": { "type": "integer" }
        },
        "required": ["name", "project", "owner", "branch", "baseRevision", "scope"]
    })
}

fn list_schema() -> Value {
    json!({ "type": "object", "properties": { "items": { "type": "array", "items": workspace_schema() } }, "required": ["items"] })
}

fn comparison_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "files": { "type": "array", "items": { "type": "object" } },
            "conflicts": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["files", "conflicts"]
    })
}

fn update_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "taken": { "type": "array", "items": { "type": "string" } },
            "merged": { "type": "array", "items": { "type": "string" } },
            "baseRevision": { "type": "string" },
            "comparison": comparison_schema()
        },
        "required": ["taken", "merged", "baseRevision", "comparison"]
    })
}

fn change_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "changeId": { "type": "string" },
            "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
            "url": { "type": "string" },
            "change": { "type": "object" }
        },
        "required": ["changeId", "lane", "change"]
    })
}

fn discarded_schema() -> Value {
    json!({ "type": "object", "properties": { "discarded": { "type": "string" } }, "required": ["discarded"] })
}

fn annotations(read_only: bool, destructive: bool, idempotent: bool) -> crate::ops::Annotations {
    crate::ops::Annotations {
        read_only_hint: read_only,
        destructive_hint: destructive,
        idempotent_hint: idempotent,
    }
}

/// The workspace operations of the registry (API/01 §21, §22).
pub fn operations() -> Vec<crate::ops::Operation> {
    use crate::ops::{parse_input, Operation};
    vec![
        Operation {
            name: "jc_workspace_open",
            title: "Open A Workspace",
            description: "Opens a named branch of the project to change several resources in, brought back later as one Change",
            input: open_schema,
            output: workspace_schema,
            annotations: annotations(false, false, false),
            kind: "Workspace",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<OpenRequest>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let request: OpenRequest = parse_input(val)?;
                    Ok(serde_json::to_value(open(&caller.identity, state, project, request).await?)?)
                })
            },
        },
        Operation {
            name: "jc_workspace_list",
            title: "List Workspaces",
            description: "The open workspaces of the project, oldest first",
            input: empty_schema,
            output: list_schema,
            annotations: annotations(true, false, true),
            kind: "Workspace",
            verb: None,
            lane: Lane::Green,
            validate: |_| Ok(()),
            run: |caller, state, project, _| {
                Box::pin(async move {
                    Ok(serde_json::to_value(list(&caller.identity, state, project).await?)?)
                })
            },
        },
        Operation {
            name: "jc_workspace_get",
            title: "Read A Workspace",
            description: "One workspace: whose it is, what it covers, when it expires and how many files it changes",
            input: name_schema,
            output: workspace_schema,
            annotations: annotations(true, false, true),
            kind: "Workspace",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<NameInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NameInput = parse_input(val)?;
                    Ok(serde_json::to_value(get(&caller.identity, state, project, &input.name).await?)?)
                })
            },
        },
        Operation {
            name: "jc_workspace_compare",
            title: "Compare A Workspace",
            description: "Every file the workspace changes with its fields and lane, and every field main changed too",
            input: name_schema,
            output: comparison_schema,
            annotations: annotations(true, false, true),
            kind: "Workspace",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<NameInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NameInput = parse_input(val)?;
                    Ok(serde_json::to_value(compare(&caller.identity, state, project, &input.name).await?)?)
                })
            },
        },
        Operation {
            name: "jc_workspace_update_from_main",
            title: "Update A Workspace From Main",
            description: "Takes what main changed into the workspace; a field both changed needs a resolution, ours or theirs",
            input: update_schema,
            output: update_output_schema,
            annotations: annotations(false, false, false),
            kind: "Workspace",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<UpdateInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: UpdateInput = parse_input(val)?;
                    let report = update_from_main(
                        &caller.identity,
                        state,
                        project,
                        &input.name,
                        UpdateRequest {
                            resolutions: input.resolutions,
                        },
                    )
                    .await?;
                    Ok(serde_json::to_value(report)?)
                })
            },
        },
        Operation {
            name: "jc_workspace_propose",
            title: "Bring A Workspace Back",
            description: "Proposes the workspace as one Change a person approves; never for an agent (AG-82)",
            input: name_schema,
            output: change_schema,
            annotations: annotations(false, false, false),
            kind: "Workspace",
            verb: None,
            lane: Lane::Yellow,
            validate: |val| parse_input::<NameInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NameInput = parse_input(val)?;
                    let change = propose(caller, state, project, &input.name).await?;
                    Ok(crate::api::mutate::ProposeOutcome::Change(change).into_value())
                })
            },
        },
        Operation {
            name: "jc_workspace_discard",
            title: "Discard A Workspace",
            description: "Removes the workspace and its branch; nothing in it reaches main",
            input: name_schema,
            output: discarded_schema,
            annotations: annotations(false, true, true),
            kind: "Workspace",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<NameInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NameInput = parse_input(val)?;
                    discard(&caller.identity, state, project, &input.name).await?;
                    Ok(json!({ "discarded": input.name }))
                })
            },
        },
    ]
}
