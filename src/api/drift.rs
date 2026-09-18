//! Drift: what a space holds and the repository does not declare, or no longer holds as it
//! does (CC-21, CC-38, UI-25, UI-26; API/01 §20).
//!
//! - `GET  /api/v1/projects/{project}/drift` — what the last scan found, with its instant
//! - `POST /api/v1/projects/{project}/drift/{space}/{id}/revert` — write Git's entity back
//! - `POST /api/v1/projects/{project}/drift/{space}/{id}/adopt` — propose the live one
//!
//! Configuration cannot drift: every component reads it from the repository, so the repository
//! is what it is running (CC-72, and T-0421 settled it). What can is a space's seed entities,
//! and the reconciler compares them on its tick — the list is what that tick found, so a page
//! renders without waiting on a broker.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::resource::is_dns1123;
use crate::state::AppState;
use jc_core::kinds::Verb;

const API_VERSION: &str = "joinedcontext.com/v1alpha1";

/// A project the caller is a member of, or `404`. Never `403`: a route that tells the two
/// apart says which projects exist (R20).
fn member_of(state: &AppState, user: &CurrentUser, project: &str) -> Result<(), ApiError> {
    if !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    if effective.bootstrap || !effective.grants.is_empty() {
        return Ok(());
    }
    Err(ApiError::NotFound(format!("project '{project}' not found")))
}

/// A resolution is a write: revert writes the live space, adopt writes the repository, so
/// neither is a read and both are held to `propose` on `Entity` (API/01 §20).
fn may_resolve(state: &AppState, user: &CurrentUser, project: &str) -> Result<(), ApiError> {
    member_of(state, user, project)?;
    crate::permissions::for_request(state, &user.0.identity, project).check(
        "Entity",
        Verb::Propose,
        None,
    )
}

/// The drifted entity the last scan found under this space and id, or `404`.
///
/// Resolving something the scan did not report would be a write nobody asked for: the list is
/// what the operator is looking at, and it is what the two buttons act on.
fn drifted(
    state: &AppState,
    project: &str,
    space: &str,
    id: &str,
) -> Result<crate::reconciler::drift::Drifted, ApiError> {
    state
        .drift
        .of(project)
        .and_then(|found| {
            found
                .entities
                .into_iter()
                .find(|entity| entity.space == space && entity.id == id)
        })
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "the last scan reported no drift for '{id}' in space '{space}'"
            ))
        })
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/drift",
    tag = "drift",
    params(("project" = String, Path, description = "Project name")),
    responses((status = 200, description = "What the last scan found")),
)]
pub async fn list_drift(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(project): Path<String>,
) -> Result<Json<Value>, ApiError> {
    member_of(&state, &user, &project)?;
    let found = state.drift.of(&project);
    // `observedAt` absent means no scan has run, which is a different answer from "nothing
    // drifted" and the page says so rather than showing an empty clean list.
    let metadata = match &found {
        Some(found) => json!({ "observedAt": found.observed_at.to_rfc3339() }),
        None => json!({}),
    };
    Ok(Json(json!({
        "apiVersion": API_VERSION,
        "kind": "List",
        "metadata": metadata,
        "items": found.map(|found| found.entities).unwrap_or_default(),
    })))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/drift/{space}/{id}/revert",
    tag = "drift",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "The Context Space holding the entity"),
        ("id" = String, Path, description = "The entity's URN, percent-encoded"),
    ),
    responses((status = 204, description = "Git's entity written back into the space")),
)]
pub async fn revert_drift(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((project, space, id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    may_resolve(&state, &user, &project)?;
    let entity = drifted(&state, &project, &space, &id)?;
    let watch = state.drift_watch.as_ref().ok_or_else(|| {
        ApiError::Unavailable(
            "no space surface is configured, so nothing can be written back".to_owned(),
        )
    })?;
    // What the file declares, and only that: revert never deletes an entity or an attribute,
    // so an attribute a device wrote beside a seeded one survives it (CC-19).
    watch
        .put(&space, &entity.declared)
        .await
        .map_err(ApiError::Unavailable)?;
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.clone(),
        space: Some(space.clone()),
        kind: "config.drifted".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} reverted '{id}' to what the repository declares",
            user.0.identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({ "object": id, "resolution": "revert", "source": entity.source }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(entity = %id, error = %err, "the revert is not on the activity feed");
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/drift/{space}/{id}/adopt",
    tag = "drift",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "The Context Space holding the entity"),
        ("id" = String, Path, description = "The entity's URN, percent-encoded"),
    ),
    responses((status = 202, description = "The live entity proposed as the seed file's content")),
)]
pub async fn adopt_drift(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((project, space, id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    may_resolve(&state, &user, &project)?;
    let entity = drifted(&state, &project, &space, &id)?;
    if entity.drift == crate::reconciler::drift::Kind::Missing {
        // Adopting "it is gone" means proposing an empty file, which is a deletion and goes
        // through the explicit-deletion path rather than through a button (CC-19, UI-26).
        return Err(ApiError::Conflict(format!(
            "'{id}' is not in the space, so there is no live entity to adopt; \
             revert writes it back, and removing it from the repository is an explicit deletion"
        )));
    }
    let watch = state.drift_watch.as_ref().ok_or_else(|| {
        ApiError::Unavailable(
            "no space surface is configured, so the live entity cannot be read".to_owned(),
        )
    })?;
    let live = watch
        .read(&space, &id)
        .await
        .map_err(ApiError::Unavailable)?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "'{id}' is no longer in the space, so there is nothing to adopt"
            ))
        })?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "apiVersion": API_VERSION,
            "kind": "Change",
            "metadata": { "project": project, "space": space },
            "spec": {
                "summary": format!("adopt the live '{id}'"),
                "path": entity.source,
                "entity": live,
            },
        })),
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/drift", get(list_drift))
        .route(
            "/projects/{project}/drift/{space}/{id}/revert",
            post(revert_drift),
        )
        .route(
            "/projects/{project}/drift/{space}/{id}/adopt",
            post(adopt_drift),
        )
}
