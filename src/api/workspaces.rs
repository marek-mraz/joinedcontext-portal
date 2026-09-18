//! Workspaces over REST (API/01 §22, T-1236): each route calls the one implementation in
//! `crate::ops::workspaces`, which the operations of the registry and MCP call too.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::auth::session::Front;
use crate::auth::CurrentUser;
use crate::change::Change;
use crate::error::{ApiError, ProblemDetails};
use crate::ops::previews::{self, Preview, ServedList};
use crate::ops::workspaces::{
    self, Comparison, OpenRequest, UpdateReport, UpdateRequest, WorkspaceList, WorkspaceView,
};
use crate::ops::{Caller, OpError, Via};
use crate::state::AppState;

fn caller(user: CurrentUser, front: Front) -> Caller {
    Caller::new(
        user.0.identity,
        match front {
            Front::Portal | Front::Edge => Via::Session,
            Front::Bearer => Via::Bearer,
        },
    )
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/workspaces",
    tag = "workspaces",
    params(("project" = String, Path, description = "Project name")),
    request_body = OpenRequest,
    responses(
        (status = 201, description = "The workspace, opened", body = WorkspaceView),
        (status = 400, description = "A name, title, scope or TTL out of bounds", body = ProblemDetails),
        (status = 403, description = "No role with propose in the project", body = ProblemDetails),
        (status = 404, description = "No such project", body = ProblemDetails),
        (status = 409, description = "A workspace of that name exists", body = ProblemDetails),
    )
)]
pub async fn open_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<OpenRequest>,
) -> Result<Response, ApiError> {
    let view = workspaces::open(&user.0.identity, &state, &project, request).await?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/workspaces",
    tag = "workspaces",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The open workspaces", body = WorkspaceList),
        (status = 404, description = "No such project", body = ProblemDetails),
    )
)]
pub async fn list_workspaces(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<WorkspaceList>, ApiError> {
    Ok(Json(
        workspaces::list(&user.0.identity, &state, &project).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/workspaces/{name}",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 200, description = "The workspace", body = WorkspaceView),
        (status = 404, description = "No such project or workspace", body = ProblemDetails),
    )
)]
pub async fn get_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<WorkspaceView>, ApiError> {
    Ok(Json(
        workspaces::get(&user.0.identity, &state, &project, &name).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/workspaces/{name}/compare",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 200, description = "What it changes, and where main changed the same", body = Comparison),
        (status = 404, description = "No such project or workspace", body = ProblemDetails),
    )
)]
pub async fn compare_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Comparison>, ApiError> {
    Ok(Json(
        workspaces::compare(&user.0.identity, &state, &project, &name).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/workspaces/{name}/update",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    request_body = UpdateRequest,
    responses(
        (status = 200, description = "Main merged into the workspace", body = UpdateReport),
        (status = 403, description = "Not the owner", body = ProblemDetails),
        (status = 409, description = "A conflict without a resolution"),
    )
)]
pub async fn update_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(request): Json<UpdateRequest>,
) -> Result<Json<UpdateReport>, OpError> {
    Ok(Json(
        workspaces::update_from_main(&user.0.identity, &state, &project, &name, request).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/workspaces/{name}/propose",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 202, description = "The Change that brings the workspace back", body = Change),
        (status = 403, description = "Not the owner, or a kind the owner may not propose", body = ProblemDetails),
        (status = 409, description = "A conflict, nothing to bring back, or already brought back"),
    )
)]
pub async fn propose_workspace(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, OpError> {
    let change = workspaces::propose(&caller(user, front), &state, &project, &name).await?;
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/workspaces/{name}",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 204, description = "The workspace and its branch are gone"),
        (status = 403, description = "Not the owner", body = ProblemDetails),
        (status = 404, description = "No such project or workspace", body = ProblemDetails),
    )
)]
pub async fn discard_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    workspaces::discard(&user.0.identity, &state, &project, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/workspaces/{name}/preview",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 202, description = "The preview, running", body = Preview),
        (status = 403, description = "Not the workspace's owner", body = ProblemDetails),
        (status = 404, description = "No such workspace", body = ProblemDetails),
        (status = 409, description = "Running already, two run on the node, or the render is refused", body = ProblemDetails),
    )
)]
pub async fn start_workspace_preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let preview = previews::start(&user.0.identity, &state, &project, &name).await?;
    Ok((StatusCode::ACCEPTED, Json(preview)).into_response())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/workspaces/{name}/preview",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 200, description = "The preview", body = Preview),
        (status = 404, description = "No such workspace", body = ProblemDetails),
    )
)]
pub async fn get_workspace_preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Preview>, ApiError> {
    Ok(Json(
        previews::get(&user.0.identity, &state, &project, &name).await?,
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/workspaces/{name}/preview",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 204, description = "The preview stopped, or was not running"),
        (status = 403, description = "Not the workspace's owner", body = ProblemDetails),
        (status = 404, description = "No such workspace", body = ProblemDetails),
    )
)]
pub async fn stop_workspace_preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    previews::stop(&user.0.identity, &state, &project, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every running preview for the gateway, on the internal listener only (Architecture/06
/// §7.2). Manifests hold `secretRef`s and no secret, and the NetworkPolicy admits the gateway
/// alone to this port.
pub async fn served_previews(State(state): State<AppState>) -> Result<Json<ServedList>, ApiError> {
    Ok(Json(previews::served(&state).await?))
}

pub fn internal_router() -> Router<AppState> {
    Router::new().route("/internal/previews", get(served_previews))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/workspaces",
            get(list_workspaces).post(open_workspace),
        )
        .route(
            "/projects/{project}/workspaces/{name}",
            get(get_workspace).delete(discard_workspace),
        )
        .route(
            "/projects/{project}/workspaces/{name}/compare",
            get(compare_workspace),
        )
        .route(
            "/projects/{project}/workspaces/{name}/update",
            post(update_workspace),
        )
        .route(
            "/projects/{project}/workspaces/{name}/preview",
            get(get_workspace_preview)
                .post(start_workspace_preview)
                .delete(stop_workspace_preview),
        )
        .route(
            "/projects/{project}/workspaces/{name}/propose",
            post(propose_workspace),
        )
}
