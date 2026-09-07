//! What a `SyncSource` reports and what an operator can do to it (MF-28, MF-30).
//!
//! The manifest itself is served by the generic resource routes like every other kind; this is
//! the running loop around it — the phase, the revision it carries, the merge request it is
//! waiting on, and the three buttons the project page offers.
//!
//! The three differ in what they touch, which is the whole reason there are three:
//!
//! - **Sync now** runs the loop once, whatever the schedule says. It changes nothing but the
//!   run's memory, and what it produces is a merge request like any other run's.
//! - **Pause** switches the loop off. It is an operator's decision about a running process, not
//!   a change to the repository, so it is recorded beside the run's memory and no merge
//!   request is opened for it.
//! - **Detach** stops syncing for good, and that *is* a change to the repository: the manifest
//!   goes away, so the change is proposed and reviewed like every other one (CC-03). The
//!   resources the source brought in stay where they are — detaching a source is not deleting
//!   what it published.
//!
//! The webhook route lives here too. A source with `schedule: { webhook: true }` runs when its
//! origin says it moved, and the caller is authenticated the same way the forge callback is: an
//! HMAC-SHA256 signature over the body, against the secret the Portal already holds. An
//! unauthenticated trigger would be a way for anybody on the internet to make the Portal fetch
//! a remote repository as often as they liked.

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource;
use crate::state::AppState;
use crate::sync::driver::{Driver, RunError};
use crate::sync::proposal::{self, Proposal};
use crate::sync::state::Stored;

/// The kind this module drives.
const KIND: &str = "SyncSource";

/// What the project page shows for one source (MF-30).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SyncSourceStatus {
    pub project: String,
    pub name: String,
    /// `Synced`, `OutOfSync`, `PendingApproval`, `Error` or `Paused`.
    pub phase: String,
    /// The source revision the repository carries.
    pub observed_revision: Option<String>,
    /// When the last run happened, in seconds since the epoch.
    pub last_run_at: Option<u64>,
    /// The merge request a run opened and nobody has answered yet.
    pub merge_request: Option<String>,
    /// Why the last run did not finish, when it did not.
    pub last_error: Option<String>,
    pub paused: bool,
    /// Whether a restart keeps this. `false` on a Portal without a database, where a restart
    /// costs one duplicate proposal per source with a run in flight.
    pub durable: bool,
}

impl SyncSourceStatus {
    fn of(project: &str, name: &str, stored: &Stored, durable: bool) -> Self {
        Self {
            project: project.to_owned(),
            name: name.to_owned(),
            phase: stored.phase().as_str().to_owned(),
            observed_revision: stored.state.observed_revision.clone(),
            last_run_at: stored.state.last_run_at,
            merge_request: stored.merge_request.clone(),
            last_error: stored.last_error.clone(),
            paused: stored.state.paused,
            durable,
        }
    }
}

/// What one run did, and where the source stands afterwards.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SyncRunReport {
    /// The `kind: Change` envelope of every merge request the run opened.
    pub proposed: Vec<Value>,
    /// Whether the run changed anything at all.
    pub unchanged: usize,
    /// What the run could not do, in sentences a person can act on.
    pub flags: Vec<String>,
    pub status: SyncSourceStatus,
}

/// Whether syncing is on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
pub struct PauseRequest {
    /// `true` switches the loop off, `false` switches it back on.
    pub paused: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/syncsources/{name}/status",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
    ),
    responses(
        (status = 200, description = "What the source reports about itself", body = SyncSourceStatus),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project or source", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn status(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<SyncSourceStatus>, ApiError> {
    let driver = driver(&state)?;
    let (project, name) = named(&state, &project, &name)?;
    let stored = driver.status(&project, &name).await;
    Ok(Json(SyncSourceStatus::of(
        &project,
        &name,
        &stored,
        driver.is_durable(),
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/syncsources/{name}/sync",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
    ),
    responses(
        (status = 200, description = "The run and what the source reports afterwards", body = SyncRunReport),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project or source", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn sync_now(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<SyncRunReport>, ApiError> {
    let driver = driver(&state)?;
    let (project, name) = named(&state, &project, &name)?;
    Ok(Json(run(driver, &project, &name).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/syncsources/{name}/pause",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
    ),
    request_body = PauseRequest,
    responses(
        (status = 200, description = "What the source reports afterwards", body = SyncSourceStatus),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project or source", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn pause(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(request): Json<PauseRequest>,
) -> Result<Json<SyncSourceStatus>, ApiError> {
    let driver = driver(&state)?;
    let (project, name) = named(&state, &project, &name)?;
    driver.pause(&project, &name, request.paused).await;
    let stored = driver.status(&project, &name).await;
    Ok(Json(SyncSourceStatus::of(
        &project,
        &name,
        &stored,
        driver.is_durable(),
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/syncsources/{name}/detach",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
    ),
    responses(
        (status = 202, description = "The merge request that removes the source", body = Object),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project or source", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn detach(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let driver = driver(&state)?;
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let (project, name) = named(&state, &project, &name)?;

    let info = resource::by_kind(KIND)
        .ok_or_else(|| ApiError::Internal("the SyncSource kind is not in the catalogue".into()))?;
    let path =
        resource::repository_path(info, &project, None, &name).map_err(ApiError::BadRequest)?;

    // Off before the merge request is answered, not after: a source that kept syncing while its
    // own removal waited for review would open merge requests nobody wants to read.
    driver.pause(&project, &name, true).await;

    let pull = proposal::open(
        gitea,
        &Proposal {
            branch: &format!("detach/{project}-{name}"),
            title: &format!("detach sync source {project}/{name}"),
            body: &format!(
                "Removes `{path}`, so `{project}/{name}` stops syncing.\n\nThe resources it \
                 brought into the project stay where they are; detaching a source is not \
                 deleting what it published. Syncing is already paused, and stays paused if \
                 this is closed rather than merged.\n"
            ),
            files: BTreeMap::new(),
            removed: vec![path],
            // Never: removing the thing that keeps a project aligned with a standard is a
            // decision a person makes (CC-70).
            auto_merge: false,
        },
    )
    .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "mergeRequest": pull.url,
            "number": pull.number,
            "status": SyncSourceStatus::of(
                &project,
                &name,
                &driver.status(&project, &name).await,
                driver.is_durable(),
            ),
        })),
    )
        .into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/webhooks/sync/{project}/{name}",
    tag = "system",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
        ("x-gitea-signature" = String, Header, description = "HMAC-SHA256 of the request body"),
    ),
    request_body(
        content = String,
        description = "Whatever the origin sends; the body is what the signature covers",
        content_type = "application/json",
    ),
    responses(
        (status = 200, description = "The run the source's origin asked for", body = SyncRunReport),
        (status = 401, description = "Missing or invalid signature", body = ProblemDetails),
        (status = 404, description = "No such project or source", body = ProblemDetails),
        (status = 503, description = "No webhook secret or no repository configured", body = ProblemDetails),
    )
)]
pub async fn webhook(
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<SyncRunReport>, ApiError> {
    let secret = state
        .config
        .gitea_webhook_secret
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("webhook secret is not configured".into()))?;
    let presented = headers
        .get(crate::api::webhook::SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !crate::api::webhook::verify_signature(secret, &body, presented) {
        return Err(ApiError::Unauthorized);
    }

    let driver = driver(&state)?;
    let (project, name) = named(&state, &project, &name)?;
    Ok(Json(run(driver, &project, &name).await?))
}

/// One forced run and the status that follows it.
async fn run(driver: &Driver, project: &str, name: &str) -> Result<SyncRunReport, ApiError> {
    let now = crate::auth::session::now_unix().max(0) as u64;
    let outcome = driver.run_now(project, name, now).await.map_err(failed)?;
    let stored = driver.status(project, name).await;
    Ok(SyncRunReport {
        proposed: outcome.proposed,
        unchanged: outcome.unchanged,
        flags: outcome.flags,
        status: SyncSourceStatus::of(project, name, &stored, driver.is_durable()),
    })
}

/// The loop, or the reason there is none.
fn driver(state: &AppState) -> Result<&Driver, ApiError> {
    state
        .sync
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("the sync loop is not running".into()))
}

/// The project and source a request names, refused when the repository holds no such source.
///
/// Read from the Portal's mirror of the repository rather than from the forge: it is the same
/// view every other resource route answers from, so a source that is not in it is a source the
/// Portal does not have (MF-04).
fn named(state: &AppState, project: &str, name: &str) -> Result<(String, String), ApiError> {
    if !resource::is_dns1123(project) || !resource::is_dns1123(name) {
        return Err(ApiError::NotFound(format!(
            "sync source '{name}' not found in project '{project}'"
        )));
    }
    if state.mirror.get(project, KIND, name).is_none() {
        return Err(ApiError::NotFound(format!(
            "sync source '{name}' not found in project '{project}'"
        )));
    }
    Ok((project.to_owned(), name.to_owned()))
}

/// A run that could not happen, as the answer the caller gets.
fn failed(err: RunError) -> ApiError {
    match err {
        RunError::NoSuchSource(name, project) => ApiError::NotFound(format!(
            "sync source '{name}' not found in project '{project}'"
        )),
        // Everything else is the forge or the origin: the request was right and the Portal
        // could not act on it.
        other => ApiError::Unavailable(other.to_string()),
    }
}

/// The four routes a session drives.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/syncsources/{name}/status", get(status))
        .route(
            "/projects/{project}/syncsources/{name}/sync",
            post(sync_now),
        )
        .route("/projects/{project}/syncsources/{name}/pause", post(pause))
        .route(
            "/projects/{project}/syncsources/{name}/detach",
            post(detach),
        )
}

/// The webhook, which is a signature and not a session.
pub fn webhook_router() -> Router<AppState> {
    Router::new().route("/webhooks/sync/{project}/{name}", post(webhook))
}
