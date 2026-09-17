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

/// One quota dimension of a project: what it holds and what it may (PF-75).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub used: u32,
    /// Absent when no quota limits this dimension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    /// Every countable dimension by its manifest field name (`contextSpaces`,
    /// `residentPipelines`, `publicEndpoints`, `apps`).
    #[schema(value_type = Object)]
    pub usage: std::collections::BTreeMap<String, Usage>,
}

/// One project as the Portal holds it, with what it is using of its quota (PF-75).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetail {
    pub api_version: String,
    pub kind: String,
    #[schema(value_type = Object)]
    pub metadata: Value,
    #[schema(value_type = Object)]
    pub spec: Value,
    pub status: ProjectStatus,
}

/// `GET /api/v1/projects/{project}`: the project and what it holds of each quota, so a person
/// sees the limit before the verdict does (PF-75). A project no binding of the caller covers is
/// `404`, like every other read of it (PF-59, R20).
#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}",
    tag = "resources",
    params(("project" = String, Path, description = "Project slug")),
    responses(
        (status = 200, description = "The project and its quota usage", body = ProjectDetail),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No binding of the caller covers the project", body = ProblemDetails)
    )
)]
pub async fn get_project(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::Path(project): axum::extract::Path<String>,
) -> Result<Json<ProjectDetail>, ApiError> {
    let identity = &user.0.identity;
    let not_found = || ApiError::NotFound(format!("project '{project}' not found"));
    if !crate::permissions::for_request(&state, identity, &project).may_read_project() {
        return Err(not_found());
    }
    let manifest = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", &project);
    // A project directory the repository holds without a Project manifest is still a project:
    // its usage is real and the page should show it (PF-05).
    if manifest.is_none()
        && !state
            .mirror
            .namespaces()
            .iter()
            .any(|held| held == &project)
    {
        return Err(not_found());
    }

    let quotas = crate::quotas::effective(&state.mirror, &project);
    let limits = crate::quotas::limits(&quotas);
    let usage = crate::quotas::usage(&state.mirror, &project)
        .into_iter()
        .map(|(dimension, used)| {
            let limit = limits.get(&dimension).copied();
            (dimension, Usage { used, limit })
        })
        .collect();

    Ok(Json(ProjectDetail {
        api_version: API_VERSION.to_string(),
        kind: "Project".to_string(),
        metadata: manifest
            .as_ref()
            .and_then(|env| serde_json::to_value(&env.metadata).ok())
            .unwrap_or_else(
                || json!({ "name": project, "namespace": crate::permissions::ORG_NAMESPACE }),
            ),
        spec: manifest.map(|env| env.spec).unwrap_or(Value::Null),
        status: ProjectStatus { usage },
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list_projects).post(open_project))
        .route(
            "/projects/{project}",
            get(get_project).delete(delete_project),
        )
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
    // A name a deleted project held stays reserved for the organization's cooling period, so
    // nobody opens a project that inherits another one's URNs, dashboards and links (PF-78).
    if let Some(free) = reserved_until(&state, gitea, &name).await {
        return Err(ApiError::Conflict(format!(
            "project '{name}' was deleted and its name stays reserved until {} (PF-78)",
            free.format("%Y-%m-%d")
        )));
    }
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

// ---------------------------------------------------------------------------
// Deleting a project (PF-77, PF-78)
// ---------------------------------------------------------------------------

/// Every file the deletion of `project` removes (PF-77).
///
/// Its own tree, and the role bindings of the organization whose scope names the project or one
/// of its spaces: those live under `users/`, so a cascade that only removed `projects/{name}/`
/// would leave grants behind pointing at a project that is gone.
pub(crate) async fn deletion_plan(
    state: &AppState,
    gitea: &crate::git::GiteaClient,
    project: &str,
    git_ref: &str,
) -> Result<Vec<String>, ApiError> {
    let tree = gitea.list_tree(git_ref).await?;
    let prefix = format!("projects/{project}/");
    let mut files: Vec<String> = tree
        .iter()
        .filter(|path| path.starts_with(&prefix))
        .cloned()
        .collect();

    let spaces: std::collections::HashSet<String> = state
        .mirror
        .list(
            project,
            "ContextSpace",
            &crate::store::ListOptions::default(),
        )
        .items
        .into_iter()
        .map(|env| env.metadata.name)
        .collect();
    let info = resource::by_kind("RoleBinding")
        .ok_or_else(|| ApiError::Internal("RoleBinding is not a kind of this Portal".into()))?;
    for env in state
        .mirror
        .list(
            crate::permissions::ORG_NAMESPACE,
            "RoleBinding",
            &crate::store::ListOptions::default(),
        )
        .items
    {
        let scope = env.spec.get("scope").cloned().unwrap_or(Value::Null);
        let names_it = scope.get("project").and_then(Value::as_str) == Some(project)
            || scope
                .get("contextSpace")
                .and_then(Value::as_str)
                .is_some_and(|space| spaces.contains(space));
        if !names_it {
            continue;
        }
        let path = resource::repository_path(
            info,
            crate::permissions::ORG_NAMESPACE,
            None,
            &env.metadata.name,
        )
        .map_err(ApiError::Internal)?;
        if tree.contains(&path) {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// The `SharedSpaceReference` manifests of other projects pointing at this project's Endpoints
/// (PF-77), as `{project}/{name}`.
///
/// A reference is another project's manifest: removing it is that project's own change, so the
/// deletion waits and names what it is waiting for rather than breaking a live share.
fn live_references(state: &AppState, project: &str) -> Vec<String> {
    let slugs: std::collections::HashSet<String> = state
        .mirror
        .list(project, "Endpoint", &crate::store::ListOptions::default())
        .items
        .into_iter()
        .filter_map(|env| {
            env.spec
                .get("slug")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    if slugs.is_empty() {
        return Vec::new();
    }
    let mut naming: Vec<String> = state
        .mirror
        .matching(|env| {
            env.kind == "SharedSpaceReference"
                && env.metadata.namespace.as_deref() != Some(project)
                && env
                    .spec
                    .get("endpointSlug")
                    .and_then(Value::as_str)
                    .is_some_and(|slug| slugs.contains(slug))
        })
        .into_iter()
        .map(|env| {
            format!(
                "{}/{}",
                env.metadata.namespace.unwrap_or_default(),
                env.metadata.name
            )
        })
        .collect();
    naming.sort();
    naming
}

/// How long a deleted project's name stays reserved (PF-78): the organization's setting, else
/// the 30 days jc-core ships.
fn cooldown_days(state: &AppState) -> u64 {
    state
        .mirror
        .list(
            crate::permissions::ORG_NAMESPACE,
            "Organization",
            &crate::store::ListOptions::default(),
        )
        .items
        .iter()
        .find_map(|env| {
            env.spec
                .pointer("/projects/nameCooldownDays")
                .and_then(Value::as_u64)
        })
        .unwrap_or(u64::from(jc_core::kinds::DEFAULT_NAME_COOLDOWN_DAYS))
}

/// When the name of a project that was deleted becomes free again, if it is still reserved
/// (PF-78). `None` means nobody deleted a project of this name, or the period has passed.
///
/// The commit that removed `projects/{name}/project.yaml` is the start of the period: the forge
/// history is the record, so the reservation survives a restart and a re-sync of the mirror.
async fn reserved_until(
    state: &AppState,
    gitea: &crate::git::GiteaClient,
    name: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let days = cooldown_days(state);
    if days == 0 {
        return None;
    }
    let branch = gitea.default_branch().await.ok()?;
    let history = gitea
        .list_commits(&branch, &format!("projects/{name}/project.yaml"), 1)
        .await
        .unwrap_or_default();
    let last = history.first()?;
    let removed = chrono::DateTime::parse_from_rfc3339(&last.date).ok()?;
    let free = removed.with_timezone(&chrono::Utc) + chrono::Duration::days(days as i64);
    (chrono::Utc::now() < free).then_some(free)
}

/// `DELETE /api/v1/projects/{project}`: proposes the one red-lane change that removes a project
/// and everything written for it (PF-77).
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}",
    tag = "resources",
    params(("project" = String, Path, description = "The project to delete")),
    responses(
        (status = 202, description = "The change that deletes the project", body = Change),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not delete this project", body = ProblemDetails),
        (status = 404, description = "No such project, or none this caller may read", body = ProblemDetails),
        (status = 409, description = "A share points at it, or a deletion is already open", body = ProblemDetails),
        (status = 503, description = "No git forge configured", body = ProblemDetails)
    )
)]
pub async fn delete_project(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::Path(project): axum::extract::Path<String>,
) -> Result<(StatusCode, Json<Change>), ApiError> {
    let change = delete_project_for(&state, &user.0.identity, &project).await?;
    Ok((StatusCode::ACCEPTED, Json(change)))
}

/// The deletion itself, so the route and the operations registry propose the same change.
pub async fn delete_project_for(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
) -> Result<Change, ApiError> {
    let missing = || ApiError::NotFound(format!("project '{project}' not found"));
    let effective = crate::permissions::for_request(state, identity, project);
    // A project the caller may not read answers as a project that is not there (R20).
    if !effective.may_read_project() {
        return Err(missing());
    }
    let manifest = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", project)
        .ok_or_else(missing)?;
    let target = serde_json::to_value(&manifest).map_err(|e| ApiError::Internal(e.to_string()))?;
    effective.check("Project", jc_core::kinds::Verb::Delete, Some(&target))?;

    let referenced = live_references(state, project);
    if !referenced.is_empty() {
        return Err(ApiError::Conflict(format!(
            "project '{project}' is shared with {}: remove the SharedSpaceReference there first \
             (PF-77)",
            referenced.join(", ")
        )));
    }

    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let default_branch = gitea.default_branch().await?;
    let files = deletion_plan(state, gitea, project, &default_branch).await?;
    if files.is_empty() {
        return Err(missing());
    }

    let branch = format!("portal/delete-project-{project}");
    if let Some(pending) = crate::api::mutate::open_change_on(gitea, &branch, project).await? {
        return Err(ApiError::Conflict(format!(
            "deleting project '{project}' is already proposed: {}; approve or reject it first",
            pending.name
        )));
    }
    let branch =
        crate::api::mutate::create_or_reuse_branch(gitea, &branch, &default_branch).await?;
    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, project);
    for path in &files {
        let Some(file) = gitea.get_file(path, &branch).await? else {
            continue;
        };
        gitea
            .delete_file(&crate::git::FileDelete {
                path,
                branch: &branch,
                message: &format!("delete {path}"),
                sha: &file.sha,
                author: crate::git::Author {
                    name: &author_name,
                    email: &author_email,
                },
            })
            .await?;
    }

    let title = format!("delete project {project}");
    let listed = files
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = format!(
        "Deleting project `{project}` removes {} files, and with them every space, endpoint, \
         pipeline, app, service account, role and binding written for it (PF-77).\n\n{listed}\n\n\
         Export each space's data before approving — `GET /api/v1/projects/{project}/export` \
         while the project is still here — because the broker tenants are dropped when this \
         merges (CC-07). The name stays reserved afterwards (PF-78).",
        files.len()
    );
    let pull = gitea
        .create_pull_request(&branch, &default_branch, &title, &body)
        .await?;

    let summary = crate::change::PlanSummary::new(0, 0, files.len());
    Ok(Change::new(
        crate::change::ChangeMeta::from_merge_request(pull.number, project),
        crate::change::ChangeStatus::new(
            crate::change::Lane::Red,
            crate::change::ChangePhase::PendingApproval,
            summary,
        )
        .with_merge_request(pull.url),
    ))
}
