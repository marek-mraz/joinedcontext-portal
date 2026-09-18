//! Workspace previews (CC-78, CC-81, PF-83; API/01 §22, Architecture/06 §7.2).
//!
//! A preview is the workspace's branch rendered by the loader with the prefix `ws-{name}-`.
//! On `dev` it runs inside the shared services: the Portal holds the render, cached by the
//! branch head, and lists every running preview on its internal listener; the gateway loads
//! that list beside `main` and serves the preview's Endpoints on minted slugs. Nothing runs
//! as a workload of its own, and every pipeline stays paused.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::error::ApiError;
use crate::ops::workspaces::{self, PreviewState, Workspace};
use crate::state::AppState;

/// At most this many previews run on the node at once (ADR-N-024 §10).
pub const MAX_ON_NODE: usize = 2;

/// What the loader puts in front of every organization-unique name of the preview.
pub fn prefix_of(workspace: &str) -> String {
    format!("ws-{workspace}-")
}

/// One Endpoint of a preview and where it answers.
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEndpoint {
    /// The Endpoint's name, as in the workspace.
    pub name: String,
    /// The slug minted for the preview; never the origin's.
    pub slug: String,
    pub url: String,
    /// The slug `main` serves the same Endpoint on, the source of a copy; absent for an
    /// Endpoint the workspace adds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_slug: Option<String>,
}

/// A workspace's preview as a person, an agent or an MCP client reads it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub state: PreviewState,
    pub prefix: String,
    /// The Endpoints of the workspace's project in the preview; empty unless it runs.
    pub endpoints: Vec<PreviewEndpoint>,
    /// Every pipeline of the project, paused until a person starts it (PL-40).
    pub paused_pipelines: Vec<String>,
    /// Why the loader refused the render, when it did (CC-78).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One running preview as the gateway reads it.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Served {
    pub prefix: String,
    /// The branch's manifest files by path: the organization's files and the project's own.
    pub files: BTreeMap<String, String>,
}

/// The list on the internal listener.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ServedList {
    pub items: Vec<Served>,
}

/// What a render found in the project: its Endpoints as `(name, slug)`, and its pipelines.
type Found = (Vec<(String, String)>, Vec<String>);

/// A render of one branch head.
#[derive(Debug)]
pub struct Render {
    files: BTreeMap<String, String>,
    outcome: Result<Found, String>,
}

/// The renders the Portal holds, one per workspace, keyed by the branch head it rendered.
#[derive(Debug, Default)]
pub struct Renders(Mutex<HashMap<String, (String, Arc<Render>)>>);

impl Renders {
    fn get(&self, workspace: &str, head: &str) -> Option<Arc<Render>> {
        let held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        held.get(workspace)
            .filter(|(revision, _)| revision == head)
            .map(|(_, render)| Arc::clone(render))
    }

    fn put(&self, workspace: &str, head: String, render: Arc<Render>) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(workspace.to_owned(), (head, render));
    }

    pub(crate) fn forget(&self, workspace: &str) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(workspace);
    }
}

/// The files under `root`, by their path relative to it.
fn files_under(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                stack.push(path);
            } else if name == "bento.yaml" || jcctl::secrets::sops::is_encrypted_file(&name) {
                // A mapping the gateway never reads, and the repository's encrypted secrets,
                // which a preview's reader has no use for: neither leaves the Portal.
                continue;
            } else if let (Ok(relative), Ok(text)) =
                (path.strip_prefix(root), std::fs::read_to_string(&path))
            {
                files.insert(relative.to_string_lossy().into_owned(), text);
            }
        }
    }
    files
}

/// Renders the branch as it is now, or hands back the render of the same head.
///
/// Only the organization's own files and the workspace project's travel: the preview is the
/// project's, and another project's Endpoints get no preview copy. A render the loader
/// refuses is kept too, with its reason, so asking again does not stage the branch again.
async fn render(state: &AppState, workspace: &Workspace) -> Result<Arc<Render>, ApiError> {
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let head = match gitea.branch_head(&workspace.branch()).await {
        Ok(head) => head,
        Err(crate::git::GitError::NotFound) => {
            return Err(ApiError::NotFound(format!(
                "the branch of workspace '{}' is gone",
                workspace.name
            )))
        }
        Err(err) => return Err(err.into()),
    };
    if let Some(render) = state.previews.get(&workspace.name, &head) {
        return Ok(render);
    }
    let scratch = crate::reconciler::daemon::stage(gitea, &head)
        .await
        .map_err(|err| ApiError::Unavailable(format!("the branch did not stage: {err}")))?;
    if let Ok(projects) = std::fs::read_dir(scratch.path().join("projects")) {
        for project in projects.flatten() {
            if project.file_name().to_string_lossy() != workspace.project {
                let _ = std::fs::remove_dir_all(project.path());
            }
        }
    }
    let prefix = prefix_of(&workspace.name);
    let environment = std::env::var("JC_ENVIRONMENT")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let namespace = format!("{prefix}{}", workspace.project);
    let outcome =
        jcctl::loader::Repository::load_preview(scratch.path(), environment.as_deref(), &prefix)
            .map(|repository| {
                let mut endpoints = Vec::new();
                let mut pipelines = Vec::new();
                for (id, loaded) in repository.iter() {
                    if id.namespace.as_deref() != Some(namespace.as_str()) {
                        continue;
                    }
                    match id.kind.as_str() {
                        "Endpoint" => endpoints.push((
                            id.name.clone(),
                            loaded.manifest.spec["slug"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned(),
                        )),
                        "Pipeline" => pipelines.push(id.name.clone()),
                        _ => {}
                    }
                }
                (endpoints, pipelines)
            })
            .map_err(|err| err.to_string());
    let render = Arc::new(Render {
        files: files_under(scratch.path()),
        outcome,
    });
    state
        .previews
        .put(&workspace.name, head, Arc::clone(&render));
    Ok(render)
}

fn view(state: &AppState, workspace: &Workspace, render: Option<&Render>) -> Preview {
    let mut preview = Preview {
        state: workspace.preview_state,
        prefix: prefix_of(&workspace.name),
        endpoints: Vec::new(),
        paused_pipelines: Vec::new(),
        reason: None,
    };
    match render.map(|render| &render.outcome) {
        Some(Ok((endpoints, pipelines))) => {
            preview.endpoints = endpoints
                .iter()
                .map(|(name, slug)| PreviewEndpoint {
                    name: name.clone(),
                    slug: slug.clone(),
                    url: state
                        .config
                        .public_base_url
                        .join(&format!("/api/endpoint/{slug}"))
                        .map(String::from)
                        .unwrap_or_default(),
                    origin_slug: state
                        .mirror
                        .get(&workspace.project, "Endpoint", name)
                        .and_then(|endpoint| endpoint.spec["slug"].as_str().map(str::to_owned)),
                })
                .collect();
            preview.paused_pipelines = pipelines.clone();
        }
        Some(Err(reason)) => preview.reason = Some(reason.clone()),
        None => {}
    }
    preview
}

fn live(state: PreviewState) -> bool {
    matches!(state, PreviewState::Starting | PreviewState::Running)
}

/// Starts the preview: one per workspace, [`MAX_ON_NODE`] on the node, the owner only.
pub async fn start(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<Preview, ApiError> {
    let workspace = workspaces::owned(state, identity, project, name, "start a preview of").await?;
    if live(workspace.preview_state) {
        return Err(ApiError::Conflict(format!(
            "a preview of workspace '{name}' runs already"
        )));
    }
    let now = chrono::Utc::now();
    let running: Vec<String> = state
        .workspaces
        .previewing()
        .await?
        .into_iter()
        .filter(|other| !other.expired(now))
        .map(|other| other.name)
        .collect();
    // ponytail: counted, then set; two starts in the same instant may both pass. A row lock
    // when the node takes more than two.
    if running.len() >= MAX_ON_NODE {
        return Err(ApiError::Conflict(format!(
            "{MAX_ON_NODE} previews run on the node already ({}); stop one first",
            running.join(", ")
        )));
    }
    state
        .workspaces
        .set_preview_state(name, PreviewState::Starting)
        .await?;
    let render = match render(state, &workspace).await {
        Ok(render) => render,
        Err(err) => {
            state
                .workspaces
                .set_preview_state(name, PreviewState::Error)
                .await?;
            return Err(err);
        }
    };
    let next = match &render.outcome {
        Ok(_) => PreviewState::Running,
        Err(_) => PreviewState::Error,
    };
    state.workspaces.set_preview_state(name, next).await?;
    let workspace = Workspace {
        preview_state: next,
        ..workspace
    };
    let preview = view(state, &workspace, Some(&render));
    match &preview.reason {
        Some(reason) => Err(ApiError::Conflict(format!(
            "the preview of workspace '{name}' does not render: {reason}"
        ))),
        None => Ok(preview),
    }
}

/// The preview as it is: its addresses while it runs, the loader's reason when it failed.
pub async fn get(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<Preview, ApiError> {
    let workspace = workspaces::visible(state, identity, project, name).await?;
    let render = match workspace.preview_state {
        PreviewState::Running | PreviewState::Error => render(state, &workspace).await.ok(),
        _ => None,
    };
    Ok(view(state, &workspace, render.as_deref()))
}

/// Stops the preview; the gateway drops its Endpoints on its next fetch. Stopping one that
/// does not run changes nothing.
pub async fn stop(
    identity: &Identity,
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<(), ApiError> {
    let workspace =
        workspaces::owned(state, identity, project, name, "stop the preview of").await?;
    if matches!(
        workspace.preview_state,
        PreviewState::None | PreviewState::Stopped
    ) {
        return Ok(());
    }
    state
        .workspaces
        .set_preview_state(name, PreviewState::Stopped)
        .await?;
    state.previews.forget(name);
    Ok(())
}

/// Every running preview that has not expired, rendered, for the gateway (CC-81: an expired
/// or discarded workspace is not in the list, so its Endpoints go on the next fetch).
pub async fn served(state: &AppState) -> Result<ServedList, ApiError> {
    let now = chrono::Utc::now();
    let mut items = Vec::new();
    for workspace in state.workspaces.previewing().await? {
        if workspace.expired(now) || workspace.preview_state != PreviewState::Running {
            continue;
        }
        match render(state, &workspace).await {
            Ok(render) if render.outcome.is_ok() => items.push(Served {
                prefix: prefix_of(&workspace.name),
                files: render.files.clone(),
            }),
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(workspace = %workspace.name, error = %err, "a running preview did not render")
            }
        }
    }
    Ok(ServedList { items })
}
