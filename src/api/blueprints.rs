//! The flow gallery and the flow it starts (T-0207, T-0208, CC-24, CC-30, CC-32, CC-59).
//!
//! A blueprint is an organization-level manifest, so it is not under
//! `/api/v1/projects/{project}/…` like the rest; API/01 §4 lists it among the kinds served at
//! `/api/v1/{plural}` and §13 describes the flow. Only the blueprint plural is routed here
//! rather than a wildcard `{plural}`: a wildcard segment at the root of `/api/v1` sits beside
//! `/preferences`, `/sync` and `/health`, and the gallery does not need that risk.

use std::hash::{Hash, Hasher};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use jc_core::kinds::RiskClass;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::api::mutate::{
    author_credentials, create_or_reuse_branch, find_literal_secret, resolve_repo_path,
};
use crate::api::resources::{ListMeta, ResourceList};
use crate::auth::session::CurrentUser;
use crate::change::{
    self, Change, ChangeMeta, ChangePhase, ChangeStatus, Lane, Operation, PlanSummary,
};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, FileWrite};
use crate::plan;
use crate::resource::{self, ResourceEnvelope, API_VERSION};
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
/// Fail-closed: `spec.allowedRoles` must name at least one role, which `jc_core::BlueprintSpec`
/// already refuses to accept empty, so a blueprint that reaches the mirror without one is
/// malformed rather than public. Reading it as "everyone" would turn a broken manifest into an
/// open door.
fn may_run(user: &CurrentUser, envelope: &ResourceEnvelope) -> bool {
    allowed_roles(envelope)
        .iter()
        .any(|role| user.0.identity.roles.iter().any(|r| r == role))
}

/// The lane a blueprint declares for its own changes (CC-59, CC-63).
fn declared_lane(risk: RiskClass) -> Lane {
    match risk {
        RiskClass::Green => Lane::Green,
        RiskClass::Yellow => Lane::Yellow,
        RiskClass::Red => Lane::Red,
    }
}

/// The stricter of two lanes.
///
/// A blueprint declares a lane, but a template can render a kind that is Red on its own terms
/// (a Policy, a federation edge, anything `change::classify` calls Red). Taking the stricter of
/// the two stops a Green blueprint from being a way to auto-approve a Red manifest (CC-63).
fn stricter(a: Lane, b: Lane) -> Lane {
    match (a, b) {
        (Lane::Red, _) | (_, Lane::Red) => Lane::Red,
        (Lane::Yellow, _) | (_, Lane::Yellow) => Lane::Yellow,
        _ => Lane::Green,
    }
}

/// One branch per (project, blueprint, version, parameters), so a retry of the same submission
/// reuses its branch instead of opening a second merge request (same rule as `mutate::branch_name`).
fn flow_branch(
    project: &str,
    blueprint: &str,
    version: &str,
    parameters: &serde_json::Value,
) -> String {
    let mut hasher = std::hash::DefaultHasher::new();
    (project, blueprint, version, parameters.to_string()).hash(&mut hasher);
    format!("portal/flow-{blueprint}-{:016x}", hasher.finish())[..]
        .chars()
        .take(120)
        .collect()
}

/// Turns one rendered manifest into an envelope this project may actually receive.
///
/// The checks are `mutate::propose`'s, for the same reason: expansion is not a way past them.
/// A template that renders a foreign namespace, an organization-level kind, a `status` block or
/// a literal secret is refused here rather than committed (MF-04, MF-24).
fn accept_rendered(
    yaml: &str,
    template: &str,
    project: &str,
) -> Result<(ResourceEnvelope, &'static resource::KindInfo), ApiError> {
    let body: serde_json::Value = serde_yaml_ng::from_str(yaml).map_err(|e| {
        ApiError::Internal(format!("template `{template}` rendered invalid yaml: {e}"))
    })?;
    let mut envelope: ResourceEnvelope = serde_json::from_value(body.clone()).map_err(|e| {
        ApiError::Internal(format!(
            "template `{template}` did not render a manifest: {e}"
        ))
    })?;

    if envelope.api_version != API_VERSION {
        return Err(ApiError::Internal(format!(
            "template `{template}` rendered apiVersion '{}' (expected '{API_VERSION}')",
            envelope.api_version
        )));
    }
    let kind_info = resource::by_kind(&envelope.kind).ok_or_else(|| {
        ApiError::Internal(format!(
            "template `{template}` rendered unknown kind '{}'",
            envelope.kind
        ))
    })?;

    match envelope.metadata.namespace.as_deref() {
        None | Some("") => envelope.metadata.namespace = Some(project.to_string()),
        Some(ns) if ns == project => {}
        Some(foreign) => {
            return Err(ApiError::BadRequest(format!(
                "template `{template}` renders into namespace '{foreign}', not project '{project}'"
            )))
        }
    }
    resource::validate_meta(&envelope.metadata).map_err(ApiError::BadRequest)?;

    if body.get("status").is_some() || envelope.status.is_some() {
        return Err(ApiError::Internal(format!(
            "template `{template}` rendered a status block, which the platform computes (MF-04)"
        )));
    }
    if let Some(key) = find_literal_secret(&body) {
        return Err(ApiError::BadRequest(format!(
            "template `{template}` rendered a literal secret in field '{key}'; use secretRef (MF-24)"
        )));
    }

    Ok((envelope, kind_info))
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
        (status = 400, description = "The parameters do not satisfy the blueprint's schema", body = ProblemDetails),
        (status = 409, description = "The form was filled against another version", body = ProblemDetails),
        (status = 503, description = "The git forge is not configured", body = ProblemDetails)
    )
)]
pub async fn start_flow(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<FlowRequest>,
) -> Result<Response, ApiError> {
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

    // Expansion is `jcctl`'s, the same code the reconciler runs, so the manifests in the merge
    // request are byte-identical to the ones the reconciler would render (CC-25). A second
    // engine in the Portal is exactly what that requirement forbids.
    let mut typed = envelope.clone();
    typed.strip_status();
    let blueprint: jc_core::kinds::Blueprint = serde_json::to_value(&typed)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| {
            ApiError::Internal(format!(
                "blueprint '{}' in Git is not a valid Blueprint manifest",
                request.blueprint
            ))
        })?;

    let rendered =
        jcctl::blueprints::expand(&blueprint, &request.parameters).map_err(|e| match e {
            // CC-24: every violation at once, in `errors[]`, so the form marks all its bad fields
            // in one pass rather than sending the user round the loop once per mistake.
            jcctl::blueprints::ExpandError::Parameters(violations) => ApiError::Invalid {
                detail: format!(
                    "the parameters do not match blueprint '{}': {}",
                    request.blueprint,
                    violations.join("; ")
                ),
                errors: violations,
            },
            // The blueprint itself is broken; the user filled in nothing wrong.
            other => ApiError::Internal(other.to_string()),
        })?;

    // Each rendered manifest goes through the same gate a hand-written one does, and the lane is
    // the stricter of what the blueprint declares and what the kinds themselves are (CC-63).
    let mut lane = declared_lane(blueprint.spec.risk_class);
    let mut summary = PlanSummary::default();
    let mut files = Vec::with_capacity(rendered.len());
    for expanded in &rendered {
        let (mut manifest, kind_info) =
            accept_rendered(&expanded.manifest, &expanded.template, &project)?;
        let operation = if state
            .mirror
            .get(&project, kind_info.kind, &manifest.metadata.name)
            .is_some()
        {
            Operation::Update
        } else {
            Operation::Create
        };
        lane = stricter(
            lane,
            change::classify(kind_info.kind, operation, &manifest.spec),
        );

        let current = state
            .mirror
            .get(&project, kind_info.kind, &manifest.metadata.name);
        let diff = plan::diff(current.as_ref(), Some(&manifest));
        summary.create += diff.summary.create;
        summary.update += diff.summary.update;
        summary.delete += diff.summary.delete;

        let repo_path = resolve_repo_path(&manifest, kind_info, &project)?;
        manifest.strip_status();
        let yaml = serde_yaml_ng::to_string(&manifest)
            .map_err(|e| ApiError::Internal(format!("serialize manifest to yaml: {e}")))?;
        files.push((repo_path, yaml));
    }

    // One merge request for the whole expansion: the manifests of one flow are reviewed and
    // merged together or not at all (CC-32).
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let default_branch = gitea.default_branch().await?;
    let branch = flow_branch(
        &project,
        &request.blueprint,
        &request.version,
        &request.parameters,
    );
    create_or_reuse_branch(gitea, &branch, &default_branch).await?;

    let (author_name, author_email) = author_credentials(&user, &project);
    for (repo_path, yaml) in &files {
        let existing_sha = gitea
            .get_file(repo_path, &branch)
            .await
            .ok()
            .flatten()
            .map(|f| f.sha);
        let message = format!(
            "run blueprint {} {} in {project}",
            request.blueprint, request.version
        );
        gitea
            .put_file(&FileWrite {
                path: repo_path,
                branch: &branch,
                message: &message,
                content: yaml,
                sha: existing_sha.as_deref(),
                author: Author {
                    name: &author_name,
                    email: &author_email,
                },
            })
            .await?;
    }

    let title = format!(
        "run blueprint {} {} in {project}",
        request.blueprint, request.version
    );
    let body = format!(
        "Blueprint `{}` version {} expanded into {} manifest(s) in project `{project}` via joinedcontext Portal.",
        request.blueprint,
        request.version,
        files.len()
    );
    let pr = gitea
        .create_pull_request(&branch, &default_branch, &title, &body)
        .await?;

    let change = Change::new(
        ChangeMeta::from_merge_request(pr.number, &project),
        ChangeStatus::new(lane, ChangePhase::PendingApproval, summary).with_merge_request(pr.url),
    );
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/blueprints", get(list_blueprints))
        .route("/projects/{project}/flows", post(start_flow))
}
