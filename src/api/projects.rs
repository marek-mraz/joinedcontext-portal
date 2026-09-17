use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::change::Change;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::{self, API_VERSION};
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
    Router::new().route("/projects", get(list_projects).post(open_project))
}

// ---------------------------------------------------------------------------
// Opening a project (PF-65, PF-66, PF-67, T-0869)
// ---------------------------------------------------------------------------

/// What the organization's own manifest says about who may open a project (PF-65).
///
/// `anyone` is every signed-in person, `group:<name>` the members of one `Group`, and
/// `org-admin` — the default — whoever holds `propose` on `Project`. The setting is read
/// before a `Change` exists, because a person who may not open a project must not be able to
/// open a merge request that says they did (PF-65).
pub fn may_open(
    state: &AppState,
    identity: &crate::auth::session::Identity,
) -> Result<(), ApiError> {
    let creation = state
        .mirror
        .list(
            crate::permissions::ORG_NAMESPACE,
            "Organization",
            &crate::store::ListOptions::default(),
        )
        .items
        .into_iter()
        .find_map(|env| {
            env.spec
                .pointer("/projects/creation")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "org-admin".to_owned());

    match creation.as_str() {
        "anyone" => Ok(()),
        group if group.starts_with("group:") => {
            let name = group.trim_start_matches("group:");
            if in_group(state, identity, name) {
                Ok(())
            } else {
                Err(ApiError::Denied(format!(
                    "opening a project here is for the members of group '{name}' (PF-65)"
                )))
            }
        }
        _ => crate::permissions::for_request(state, identity, crate::permissions::ORG_NAMESPACE)
            .check("Project", jc_core::kinds::Verb::Propose, None)
            .map_err(|_| {
                ApiError::Denied(
                    "opening a project here needs propose on Project, which org-admin holds \
                     (PF-65)"
                        .to_owned(),
                )
            }),
    }
}

/// The same answer as [`may_open`], in the shape `permissions/me` carries to the UI: the "New
/// project" control is enabled or disabled with this reason, and never hidden (UI-44, PF-65).
pub fn creation_affordance(
    state: &AppState,
    identity: &crate::auth::session::Identity,
) -> crate::permissions::Affordance {
    match may_open(state, identity) {
        Ok(()) => crate::permissions::Affordance {
            allowed: true,
            reason: None,
        },
        Err(err) => crate::permissions::Affordance {
            allowed: false,
            reason: Some(err.to_string()),
        },
    }
}

/// Whether the caller is a member of the named group: the `Group` manifest first, which is the
/// configuration (PF-62), and the identity provider's own groups as long as no manifest names
/// them (PF-63 is what makes the two agree).
fn in_group(state: &AppState, identity: &crate::auth::session::Identity, name: &str) -> bool {
    if let Some(group) = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Group", name)
    {
        let members = group
            .spec
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        return members.iter().any(|member| {
            let named = member
                .get("user")
                .and_then(Value::as_str)
                .or_else(|| member.as_str())
                .unwrap_or_default();
            !named.is_empty()
                && (named.eq_ignore_ascii_case(&identity.username)
                    || identity
                        .email
                        .as_deref()
                        .is_some_and(|email| named.eq_ignore_ascii_case(email)))
        });
    }
    identity.groups.iter().any(|held| held == name)
}

/// What a person fills in to open a project.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OpenProject {
    /// The slug: the `{project}` segment of every path of it (PF-67).
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `POST /api/v1/projects`: opens a project, with the opener's steward binding in the same
/// change (PF-65, PF-66, PF-67).
#[utoipa::path(
    post,
    path = "/api/v1/projects",
    tag = "resources",
    request_body = OpenProject,
    responses(
        (status = 202, description = "The change that opens the project", body = Change),
        (status = 400, description = "The name is not a DNS-1123 label", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The organization does not let this caller open a project", body = ProblemDetails),
        (status = 409, description = "A project of that name exists or is already proposed", body = ProblemDetails),
        (status = 503, description = "No git forge configured", body = ProblemDetails)
    )
)]
pub async fn open_project(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(request): Json<OpenProject>,
) -> Result<(StatusCode, Json<Change>), ApiError> {
    let identity = &user.0.identity;
    may_open(&state, identity)?;

    let name = request.name.trim().to_owned();
    if !resource::is_dns1123(&name) {
        return Err(ApiError::BadRequest(format!(
            "project name '{name}' is not a DNS-1123 label: lowercase letters, digits and \
             hyphens, starting and ending with a letter or a digit (PF-67)"
        )));
    }
    if name == crate::permissions::ORG_NAMESPACE {
        return Err(ApiError::BadRequest(format!(
            "'{name}' is the organization's own namespace and is not a project (PF-67)"
        )));
    }
    if state.mirror.namespaces().iter().any(|held| held == &name)
        || state
            .mirror
            .get(crate::permissions::ORG_NAMESPACE, "Project", &name)
            .is_some()
    {
        return Err(ApiError::Conflict(format!(
            "project '{name}' already exists (PF-67)"
        )));
    }

    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    // A name reserved by an open change is taken, even though nothing is merged yet (PF-67).
    let open = gitea.list_pull_requests("open").await?;
    let reserved = format!("portal/create-project-{name}-");
    if open.iter().any(|pr| pr.head_branch.starts_with(&reserved)) {
        return Err(ApiError::Conflict(format!(
            "project '{name}' is already proposed and waiting for approval (PF-67)"
        )));
    }

    let files = open_project_files(&state, identity, &request, &name)?;
    // Yellow: a project and its opener's own steward binding are reviewed, not confirmed by
    // typing a name back (PF-66).
    let report = crate::api::import::ImportReport {
        created: files.iter().map(|(path, _)| path.clone()).collect(),
        replaced: Vec::new(),
        skipped: Vec::new(),
        renamed: Default::default(),
        native_files: 0,
        lane: crate::change::Lane::Yellow,
        source: None,
    };
    let mut change = crate::api::import::propose_bundle(
        &state,
        identity,
        &name,
        report,
        files,
        Some(("Project", &name)),
    )
    .await?;

    // The organization that lets anyone open a project, on an installation that does not hold
    // every change for a person, has nobody to wait for: the platform merges it and the merge
    // message says so (PF-66, PF-57).
    if lets_anyone_open(&state) && state.branding().validation == crate::branding::Validation::Lax {
        let number = crate::api::changes::parse_change_id(&change.metadata.name)?;
        gitea
            .merge(
                number,
                crate::git::MergeStyle::Squash,
                &format!(
                    "Merge change proposal {}: open project {name}\n\nMerged by the platform: \
                     the organization lets anyone open a project and this installation runs \
                     platform.validation: lax (PF-65, PF-66, PF-57)",
                    change.metadata.name
                ),
            )
            .await?;
        // Nothing is waiting for a person, so the answer says so and the UI opens the project
        // itself instead of a change nobody will approve (PF-66).
        change.status.phase = crate::change::ChangePhase::Merged;
        if let Some(syncer) = state.syncer.as_ref() {
            let syncer = syncer.clone();
            tokio::spawn(async move {
                if let Err(err) = syncer.sync_once().await {
                    tracing::warn!(error = %err, "sync after opening a project failed");
                }
            });
        }
    }

    Ok((StatusCode::ACCEPTED, Json(change)))
}

fn lets_anyone_open(state: &AppState) -> bool {
    state
        .mirror
        .list(
            crate::permissions::ORG_NAMESPACE,
            "Organization",
            &crate::store::ListOptions::default(),
        )
        .items
        .iter()
        .any(|env| {
            env.spec
                .pointer("/projects/creation")
                .and_then(Value::as_str)
                == Some("anyone")
        })
}

/// The two files a new project is: its manifest, and the binding that makes its opener the
/// steward of it and of nothing else (PF-66).
fn open_project_files(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    request: &OpenProject,
    name: &str,
) -> Result<Vec<(String, String)>, ApiError> {
    let organization = state
        .mirror
        .list(
            crate::permissions::ORG_NAMESPACE,
            "Organization",
            &crate::store::ListOptions::default(),
        )
        .items
        .into_iter()
        .next()
        .map(|env| env.metadata.name)
        .ok_or_else(|| {
            ApiError::Unavailable(
                "the organization's own manifest is not in the mirror yet; a project belongs to \
                 one (PF-66)"
                    .to_owned(),
            )
        })?;

    let mut metadata = json!({ "name": name, "namespace": crate::permissions::ORG_NAMESPACE });
    if let Some(title) = request
        .display_name
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        metadata["labels"] = json!({ "joinedcontext.com/display-name": title });
    }
    if let Some(about) = request
        .description
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        metadata["annotations"] = json!({ "joinedcontext.com/description": about });
    }
    let project = json!({
        "apiVersion": API_VERSION,
        "kind": "Project",
        "metadata": metadata,
        "spec": { "organizationRef": { "name": organization } },
    });

    // The opener gets `steward` on their own project and nothing anywhere else (PF-66, PF-52).
    let who = identity
        .email
        .clone()
        .unwrap_or_else(|| identity.username.clone());
    let binding = json!({
        "apiVersion": API_VERSION,
        "kind": "RoleBinding",
        "metadata": {
            "name": format!("{name}-creator"),
            "namespace": crate::permissions::ORG_NAMESPACE,
        },
        "spec": {
            "subjects": [{ "user": who }],
            "role": "steward",
            "scope": { "project": name },
        },
    });

    let yaml = |value: &Value| -> Result<String, ApiError> {
        serde_yaml_ng::to_string(value)
            .map_err(|e| ApiError::Internal(format!("manifest did not serialise: {e}")))
    };
    Ok(vec![
        (format!("projects/{name}/project.yaml"), yaml(&project)?),
        (
            format!("users/assignments/{name}-creator.yaml"),
            yaml(&binding)?,
        ),
    ])
}
