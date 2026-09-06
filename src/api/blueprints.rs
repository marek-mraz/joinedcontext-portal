//! The flow gallery and the flow it starts (T-0207, T-0208, CC-24, CC-30, CC-32, CC-59).
//!
//! A blueprint is an organization-level manifest, so it is not under
//! `/api/v1/projects/{project}/…` like the rest; API/01 §4 lists it among the kinds served at
//! `/api/v1/{plural}` and §13 describes the flow. Only the blueprint plural is routed here
//! rather than a wildcard `{plural}`: a wildcard segment at the root of `/api/v1` sits beside
//! `/preferences`, `/sync` and `/health`, and the gallery does not need that risk.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::resources::{ListMeta, ResourceList};
use crate::auth::session::CurrentUser;
use crate::change::Change;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::{ResourceEnvelope, API_VERSION};
use crate::state::AppState;
use crate::store::ListOptions;

/// Blueprints live in the organization namespace, like `org.yaml` (see `sync.rs`).
pub const ORG_NAMESPACE: &str = "org";
const BLUEPRINT_KIND: &str = "Blueprint";

/// `spec.allowedRoles`: the roles that may see and run a blueprint (CC-59).
fn allowed_roles(envelope: &ResourceEnvelope) -> Vec<&str> {
    envelope
        .spec
        .get("allowedRoles")
        .and_then(|value| value.as_array())
        .map(|roles| roles.iter().filter_map(|role| role.as_str()).collect())
        .unwrap_or_default()
}

/// Whether this caller may run this blueprint (CC-59).
///
/// A blueprint that names no role is available to everyone who can reach the gallery at all;
/// naming roles is how an author narrows it, and an empty list would otherwise mean "nobody"
/// by accident.
fn may_run(user: &CurrentUser, envelope: &ResourceEnvelope) -> bool {
    let allowed = allowed_roles(envelope);
    allowed.is_empty()
        || allowed
            .iter()
            .any(|role| user.0.identity.roles.iter().any(|r| r == role))
}

#[utoipa::path(
    get,
    path = "/api/v1/blueprints",
    tag = "blueprints",
    responses(
        (status = 200, description = "The blueprints this caller may run", body = ResourceList),
        (status = 401, description = "Unauthorized", body = ProblemDetails)
    )
)]
pub async fn list_blueprints(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ResourceList>, ApiError> {
    // The gallery is a screen of cards, not a paged table: an organisation publishes tens of
    // blueprints, not thousands, and a card the filter removed must not leave a gap in a page.
    let page = state
        .mirror
        .list(ORG_NAMESPACE, BLUEPRINT_KIND, &ListOptions::default());
    let items = page
        .items
        .into_iter()
        .filter(|envelope| may_run(&user, envelope))
        .collect();

    Ok(Json(ResourceList {
        api_version: API_VERSION.to_string(),
        kind: "List".to_string(),
        metadata: ListMeta {
            continue_token: None,
            remaining_item_count: None,
        },
        items,
    }))
}

/// Running a blueprint: the parameters the form collected (API/01 §13).
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FlowRequest {
    /// The organization-level Blueprint to run.
    pub blueprint: String,
    /// The version the form was generated from. A mismatch is a conflict rather than an
    /// expansion against a schema the user never saw (CC-26).
    pub version: String,
    /// The values the user filled in, validated against `spec.parameterSchema` (CC-24).
    pub parameters: serde_json::Value,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/flows",
    tag = "blueprints",
    params(("project" = String, Path, description = "Project the flow creates resources in")),
    request_body = FlowRequest,
    responses(
        (status = 202, description = "Change proposal opened", body = Change),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such blueprint for this caller", body = ProblemDetails),
        (status = 409, description = "The form was filled against another version", body = ProblemDetails),
        (status = 503, description = "This build cannot expand blueprints", body = ProblemDetails)
    )
)]
pub async fn start_flow(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<FlowRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _ = project;
    // A blueprint the caller may not run answers exactly like one that does not exist: the
    // gallery already hid it, and a different answer here would say which ones exist (R20).
    let missing = || ApiError::NotFound(format!("blueprint '{}' not found", request.blueprint));
    let envelope = state
        .mirror
        .get(ORG_NAMESPACE, BLUEPRINT_KIND, &request.blueprint)
        .filter(|envelope| may_run(&user, envelope))
        .ok_or_else(missing)?;

    // Hiding a card is not an authorisation, so the role check runs again here (CC-59); the
    // version check is next, before anything is rendered from parameters filled against a
    // schema that has since changed (CC-26).
    let current = envelope
        .spec
        .get("version")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if current != request.version {
        return Err(ApiError::Conflict(format!(
            "the form was filled against blueprint version {}, which is now {current}",
            request.version
        )));
    }

    // Expansion is `jcctl`'s, the same code the reconciler runs; a second engine in the Portal
    // is what CC-25 forbids, and rendering manifests some other way is worse than not
    // rendering them. Until the crate is a dependency this fails loudly (API/01 §13).
    Err(ApiError::Unavailable(
        "this build cannot expand blueprints: the expansion engine is not compiled in".into(),
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/blueprints", get(list_blueprints))
        .route("/projects/{project}/flows", post(start_flow))
}
