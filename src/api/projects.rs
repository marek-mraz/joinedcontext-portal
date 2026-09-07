use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::API_VERSION;
use crate::state::AppState;

/// The projects the configuration repository holds (PF-05): one per `projects/<slug>/`
/// directory the mirror has a manifest from.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectList {
    pub api_version: String,
    pub kind: String,
    pub items: Vec<ProjectSummary>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    /// The project slug, the `{project}` segment of every other path.
    pub name: String,
}

/// Gated exactly like the resource lists: a live session, nothing more. Whoever may list a
/// project's resources may learn that the project exists; a directory with nothing the
/// mirror recognises is not a project.
#[utoipa::path(
    get,
    path = "/api/v1/projects",
    tag = "resources",
    responses(
        (status = 200, description = "Projects present in the configuration repository", body = ProjectList),
        (status = 401, description = "Unauthorized", body = ProblemDetails)
    )
)]
pub async fn list_projects(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ProjectList>, ApiError> {
    let items = state
        .mirror
        .namespaces()
        .into_iter()
        .map(|name| ProjectSummary { name })
        .collect();
    Ok(Json(ProjectList {
        api_version: API_VERSION.to_string(),
        kind: "List".to_string(),
        items,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects", get(list_projects))
}
