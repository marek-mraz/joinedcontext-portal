//! `GET /api/v1/projects/{project}/permissions/me` (T-0526, PF-50): what the caller may do in
//! one project, so the UI renders only the controls the API would honour.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::permissions::{self, Effective};
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/permissions/me",
    tag = "permissions",
    params(("project" = String, Path, description = "Project slug")),
    responses(
        (status = 200, description = "The caller's effective rules in the project", body = Effective),
        (status = 401, description = "Unauthorized", body = crate::error::ProblemDetails),
    )
)]
pub async fn permissions_me(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<Effective>, ApiError> {
    Ok(Json(permissions::for_request(
        &state,
        &user.0.identity,
        &project,
    )))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/permissions/me", get(permissions_me))
}
