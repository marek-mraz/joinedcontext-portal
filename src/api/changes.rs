//! Change proposals and approvals API (CC-34, CC-41, CC-63, UI-23..UI-25).
//!
//! Provides the Portal's view of open merge requests so reviewers and domain
//! stewards never need to switch to the forge to review and approve proposals.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::change::{self, Change, ChangeMeta, ChangePhase, ChangeStatus, Lane, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{GitError, GiteaClient, MergeStyle, PullRequest, ReviewEvent};
use crate::plan::{self, FieldChange, PlanDiff};
use crate::resource::ResourceEnvelope;
use crate::state::AppState;

/// Human-readable proposal summary parameters derived from the plan diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    pub key: String,
    #[schema(value_type = Object)]
    pub params: serde_json::Map<String, Value>,
}

/// Human author attributed on the change proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeAuthor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// A change proposal representing an open Git merge request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeProposal {
    pub api_version: String,
    pub kind: String,
    pub metadata: ChangeMeta,
    pub status: ChangeStatus,
    pub summary: ChangeSummary,
    pub author: ChangeAuthor,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_fields: Option<Vec<FieldChange>>,
    /// Every file the merge request changes, on the detail of one change (T-0861). Absent in
    /// a listing, which carries `fileCount` instead: the list would otherwise read every
    /// file of every open change to render a number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<ChangeFile>>,
    /// How many files the merge request changes, the headline manifest included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<usize>,
}

impl ChangeProposal {
    pub const KIND: &'static str = "Change";
}

/// One file of the merge request behind a change (T-0861, MF-21, CC-63).
///
/// A bundle merge request carries more than its headline: the approval walks every file and
/// the strictest lane among them decides the confirmation (T-0832), so the page has to show
/// the same list. What the approver reads is what the server checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeFile {
    /// Path in the configuration repository.
    pub path: String,
    /// The manifest's kind, or the kind the directory names for a native file beside one.
    pub kind: String,
    /// What the merge request does to it, from the forge's own diff status.
    pub operation: Operation,
    /// The lane this file alone would take.
    pub lane: Lane,
}

/// Collection envelope for change proposals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeList {
    pub api_version: String,
    pub kind: String,
    pub items: Vec<ChangeProposal>,
}

impl ChangeList {
    pub const KIND: &'static str = "ChangeList";

    pub fn new(items: Vec<ChangeProposal>) -> Self {
        Self {
            api_version: crate::resource::API_VERSION.to_string(),
            kind: Self::KIND.to_string(),
            items,
        }
    }
}

/// Request body for approving or rejecting a change proposal.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApproveBody {
    #[serde(default)]
    pub confirm: Option<String>,
    /// Why a change is rejected; written on the merge request beside who rejected it.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Replaces sensitive field values with `"[REDACTED]"` (CC-06).
pub fn redact(fields: Vec<FieldChange>) -> Vec<FieldChange> {
    fields
        .into_iter()
        .map(|mut f| {
            let last_seg = f.path.rsplit('.').next().unwrap_or(&f.path);
            let clean = last_seg.split('[').next().unwrap_or(last_seg);
            if matches!(
                clean,
                "password"
                    | "token"
                    | "secret"
                    | "clientSecret"
                    | "client_secret"
                    | "apiKey"
                    | "api_key"
            ) {
                if f.from.is_some() {
                    f.from = Some(Value::String("[REDACTED]".to_string()));
                }
                if f.to.is_some() {
                    f.to = Some(Value::String("[REDACTED]".to_string()));
                }
            }
            f
        })
        .collect()
}

/// Validates and parses `{id}` formatted as `chg-` plus eight lowercase hex digits.
pub fn parse_change_id(id: &str) -> Result<u64, ApiError> {
    let invalid = || {
        ApiError::BadRequest(format!(
            "invalid change id '{id}'; expected 'chg-' followed by 8 lowercase hex digits"
        ))
    };
    let Some(hex) = id.strip_prefix("chg-") else {
        return Err(invalid());
    };
    if hex.len() != 8
        || !hex
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        return Err(invalid());
    }
    u64::from_str_radix(hex, 16).map_err(|_| invalid())
}

/// Parsed components from a `portal/{op}-{kind}-{name}-{hash}` branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub operation: Operation,
    pub kind_lower: String,
    pub resource_name: String,
    pub hash: String,
}

pub fn parse_branch_name(branch: &str) -> Option<BranchInfo> {
    let clean = branch.strip_prefix("refs/heads/").unwrap_or(branch);
    let rest = clean.strip_prefix("portal/")?;
    // A retry after a rejection carries `_{nonce}` after the hash (T-0887); the name is the same.
    let rest = match rest.rsplit_once('_') {
        Some((base, nonce)) if nonce.len() == 8 && nonce.bytes().all(|b| b.is_ascii_hexdigit()) => {
            base
        }
        _ => rest,
    };
    let mut parts: Vec<&str> = rest.split('-').collect();
    if parts.len() < 4 {
        return None;
    }
    let op = match parts[0] {
        "create" => Operation::Create,
        "update" => Operation::Update,
        "delete" => Operation::Delete,
        _ => return None,
    };
    let kind_lower = parts[1].to_string();
    let hash = parts.pop()?.to_string();
    let resource_name = parts[2..].join("-");
    if resource_name.is_empty() {
        return None;
    }
    Some(BranchInfo {
        operation: op,
        kind_lower,
        resource_name,
        hash,
    })
}

/// A caller with no `approve` grant in the project is refused before the forge is asked
/// anything: the cheap answer first, the kind-precise one once the change is loaded (PF-50).
pub(crate) fn may_approve_anything(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
) -> Result<(), ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if effective.bootstrap
        || effective
            .grants
            .iter()
            .any(|grant| grant.rule.verbs.contains(&jc_core::kinds::Verb::Approve))
    {
        return Ok(());
    }
    Err(ApiError::Denied(format!(
        "no role grants approve in project {project} (PF-50)"
    )))
}

/// Approval and rejection need `approve` on the change's kind in a binding that covers the
/// project (T-0526, PF-50); the head manifest is what the constraints are checked against,
/// the base one for a deletion.
fn may_approve(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    data: &ManifestData,
) -> Result<(), ApiError> {
    let target = data
        .head_envelope
        .as_ref()
        .or(data.base_envelope.as_ref())
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    crate::permissions::for_request(state, identity, project).check(
        &data.kind,
        jc_core::kinds::Verb::Approve,
        target.as_ref(),
    )
}

pub(crate) struct ManifestData {
    pub kind: String,
    pub name: String,
    pub operation: Operation,
    pub base_envelope: Option<ResourceEnvelope>,
    pub head_envelope: Option<ResourceEnvelope>,
}

async fn find_manifest_in_tree(
    gitea: &GiteaClient,
    git_ref: &str,
    project: &str,
    resource_name: &str,
) -> Result<Option<String>, ApiError> {
    let tree = match gitea.list_tree(git_ref).await {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };
    let prefix = format!("projects/{project}/");
    let yaml_suffix = format!("/{resource_name}.yaml");
    let yml_suffix = format!("/{resource_name}.yml");
    let space_yaml = format!("/{resource_name}/space.yaml");
    let pipeline_yaml = format!("/{resource_name}/pipeline.yaml");
    let app_yaml = format!("/{resource_name}/app.yaml");

    for p in tree {
        if p.starts_with(&prefix)
            && (p.ends_with(&yaml_suffix)
                || p.ends_with(&yml_suffix)
                || p.ends_with(&space_yaml)
                || p.ends_with(&pipeline_yaml)
                || p.ends_with(&app_yaml))
        {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

async fn load_manifest_data(
    gitea: &GiteaClient,
    pr: &PullRequest,
    project: &str,
) -> Result<Option<ManifestData>, ApiError> {
    let Some(branch_info) = parse_branch_name(&pr.head_branch) else {
        return Ok(None);
    };

    let kind_info =
        crate::resource::kinds().find(|k| k.kind.to_ascii_lowercase() == branch_info.kind_lower);

    let (base_envelope, head_envelope) = match branch_info.operation {
        Operation::Delete => {
            // A kind that lives in either place has two candidate paths; the one the base
            // branch actually carries is the manifest (PF-68).
            let candidates: Vec<String> = kind_info
                .map(|info| {
                    crate::resource::homes(info, project)
                        .into_iter()
                        .filter_map(|home| {
                            crate::resource::repository_path(
                                info,
                                &home,
                                Some(project),
                                &branch_info.resource_name,
                            )
                            .ok()
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut path = None;
            for candidate in candidates {
                if let Ok(Some(_)) = gitea.get_file(&candidate, &pr.base_branch).await {
                    path = Some(candidate);
                    break;
                }
            }

            let path = match path {
                Some(p) => Some(p),
                None => {
                    find_manifest_in_tree(
                        gitea,
                        &pr.base_branch,
                        project,
                        &branch_info.resource_name,
                    )
                    .await?
                }
            };

            let Some(repo_path) = path else {
                return Ok(None);
            };

            let base_file = gitea
                .get_file(&repo_path, &pr.base_branch)
                .await?
                .ok_or_else(|| {
                    ApiError::NotFound(format!(
                        "manifest file '{repo_path}' not found on base branch"
                    ))
                })?;

            let env: ResourceEnvelope = serde_yaml_ng::from_str(&base_file.content)
                .map_err(|e| ApiError::Internal(format!("failed to parse base manifest: {e}")))?;

            (Some(env), None)
        }
        Operation::Create | Operation::Update => {
            let direct = kind_info.and_then(|info| {
                crate::resource::repository_path(
                    info,
                    project,
                    Some(project),
                    &branch_info.resource_name,
                )
                .ok()
            });

            let path = if let Some(cand) = direct {
                if let Ok(Some(_)) = gitea.get_file(&cand, &pr.head_branch).await {
                    Some(cand)
                } else {
                    None
                }
            } else {
                None
            };

            let path = match path {
                Some(p) => Some(p),
                None => {
                    find_manifest_in_tree(
                        gitea,
                        &pr.head_branch,
                        project,
                        &branch_info.resource_name,
                    )
                    .await?
                }
            };

            let Some(repo_path) = path else {
                return Ok(None);
            };

            let head_file = gitea
                .get_file(&repo_path, &pr.head_branch)
                .await?
                .ok_or_else(|| {
                    ApiError::NotFound(format!(
                        "manifest file '{repo_path}' not found on head branch"
                    ))
                })?;

            let head_env: ResourceEnvelope = serde_yaml_ng::from_str(&head_file.content)
                .map_err(|e| ApiError::Internal(format!("failed to parse head manifest: {e}")))?;

            let base_env = if branch_info.operation == Operation::Update {
                if let Ok(Some(base_file)) = gitea.get_file(&repo_path, &pr.base_branch).await {
                    serde_yaml_ng::from_str::<ResourceEnvelope>(&base_file.content).ok()
                } else {
                    None
                }
            } else {
                None
            };

            (base_env, Some(head_env))
        }
    };

    let kind = head_envelope
        .as_ref()
        .map(|e| e.kind.clone())
        .or_else(|| base_envelope.as_ref().map(|e| e.kind.clone()))
        .unwrap_or_else(|| {
            kind_info
                .map(|k| k.kind.to_string())
                .unwrap_or_else(|| branch_info.kind_lower.clone())
        });

    let name = head_envelope
        .as_ref()
        .map(|e| e.metadata.name.clone())
        .or_else(|| base_envelope.as_ref().map(|e| e.metadata.name.clone()))
        .unwrap_or(branch_info.resource_name);

    Ok(Some(ManifestData {
        kind,
        name,
        operation: branch_info.operation,
        base_envelope,
        head_envelope,
    }))
}

/// The human behind a pull request: the author of its head commit (CC-44; the forge token only
/// posts the pull request), the poster when the branch has no commit yet or the forge does not
/// answer.
// ponytail: one forge call per open change; a cache keyed by head sha if the list grows.
async fn human_author(gitea: &GiteaClient, pr: &crate::git::gitea::PullRequest) -> ChangeAuthor {
    match gitea.list_commits(&pr.head_branch, "", 1).await {
        Ok(commits) if commits.first().is_some_and(|c| !c.author.trim().is_empty()) => {
            ChangeAuthor {
                name: commits[0].author.clone(),
                email: commits[0].email.clone(),
            }
        }
        _ => ChangeAuthor {
            name: pr.author_name.clone(),
            email: pr.author_email.clone(),
        },
    }
}

fn build_proposal(
    pr: &PullRequest,
    project: &str,
    data: &ManifestData,
    plan: PlanDiff,
    plan_fields: Option<Vec<FieldChange>>,
) -> ChangeProposal {
    let lane = if data.operation == Operation::Delete {
        Lane::Red
    } else if let Some(ref env) = data.head_envelope {
        change::classify(&env.kind, data.operation, &env.spec)
    } else {
        Lane::Yellow
    };

    let summary_key = match data.operation {
        Operation::Create => "change.summary.create",
        Operation::Update => "change.summary.update",
        Operation::Delete => "change.summary.delete",
    };

    let mut params = serde_json::Map::new();
    params.insert("kind".to_string(), Value::String(data.kind.clone()));
    params.insert("name".to_string(), Value::String(data.name.clone()));
    params.insert(
        "fields".to_string(),
        Value::Number(plan.fields.len().into()),
    );

    let summary = ChangeSummary {
        key: summary_key.to_string(),
        params,
    };

    let phase = if pr.merged {
        ChangePhase::Merged
    } else if pr.state == "closed" {
        ChangePhase::Rejected
    } else {
        ChangePhase::PendingApproval
    };

    let change_meta = ChangeMeta::from_merge_request(pr.number, project);
    let change_status =
        ChangeStatus::new(lane, phase, plan.summary).with_merge_request(pr.url.clone());

    ChangeProposal {
        api_version: crate::resource::API_VERSION.to_string(),
        kind: ChangeProposal::KIND.to_string(),
        metadata: change_meta,
        status: change_status,
        summary,
        author: ChangeAuthor {
            name: pr.author_name.clone(),
            email: pr.author_email.clone(),
        },
        created_at: pr.created_at.clone(),
        plan_fields,
        // Filled by the caller that read the merge request's files; a proposal built from the
        // headline alone says nothing about them rather than claiming there is one.
        files: None,
        file_count: None,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/changes",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
    ),
    responses(
        (status = 200, description = "List of change proposals", body = ChangeList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn list_changes(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<ChangeList>, ApiError> {
    Ok(Json(
        list_changes_readable(&state, &user.0.identity, &project).await?,
    ))
}

/// The open changes this caller may read: the ones whose resource a binding of theirs reads
/// (PF-59, T-0918).
///
/// A change carries the head and base manifests, so reading one is reading the resource. A
/// project no binding covers is `404`, the one answer for "not there" and "not yours" (R20);
/// inside a project the caller reads, a change to a kind they do not read is simply not in the
/// list, the way the resource itself is not.
pub async fn list_changes_readable(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
) -> Result<ChangeList, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let list = list_changes_for(state, project).await?;
    let items = list
        .items
        .into_iter()
        .filter(|proposal| match kind_of(proposal) {
            Some(kind) => effective.may_read(kind),
            // A change whose manifest names no kind is unreadable rather than open to all.
            None => false,
        })
        .collect();
    Ok(ChangeList::new(items))
}

/// One change this caller may read, or the `404` that says nothing about which of the two
/// reasons it was (PF-59, R20, T-0918).
pub async fn change_readable(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    id: &str,
) -> Result<ChangeProposal, ApiError> {
    let missing = || {
        ApiError::NotFound(format!(
            "change proposal '{id}' not found in project '{project}'"
        ))
    };
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() {
        return Err(missing());
    }
    let proposal = change_for(state, project, id).await?;
    match kind_of(&proposal) {
        Some(kind) if effective.may_read(kind) => Ok(proposal),
        _ => Err(missing()),
    }
}

/// The kind of the resource a change proposes, as `build_proposal` records it.
fn kind_of(proposal: &ChangeProposal) -> Option<&str> {
    proposal.summary.params.get("kind").and_then(Value::as_str)
}

/// Core proposal listing reusable by the REST route, operations registry and MCP.
pub async fn list_changes_for(state: &AppState, project: &str) -> Result<ChangeList, ApiError> {
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let prs = gitea.list_pull_requests("open").await?;
    let mut proposals = Vec::new();

    for pr in prs {
        if !pr.head_branch.starts_with("portal/") {
            continue;
        }

        let Some(data) = load_manifest_data(gitea, &pr, project).await? else {
            continue;
        };

        let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
        let mut proposal = build_proposal(&pr, project, &data, plan, None);
        proposal.author = human_author(gitea, &pr).await;
        // The count only: a listing that read every file of every open change to render
        // "+3 files" would pay for the detail page on the way past it (T-0861).
        proposal.file_count = Some(gitea.pull_request_files(pr.number).await?.len());
        proposals.push(proposal);
    }

    proposals.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(ChangeList::new(proposals))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/changes/{id}",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Change proposal ID (chg- + 8 hex digits)"),
    ),
    responses(
        (status = 200, description = "Change proposal with plan diff", body = ChangeProposal),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Change proposal not found", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn get_change(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<ChangeProposal>, ApiError> {
    Ok(Json(
        change_readable(&state, &user.0.identity, &project, &id).await?,
    ))
}

/// One change with its plan; the read behind the change route and the assistant's
/// diagnostics door (AG-57).
pub async fn change_for(
    state: &AppState,
    project: &str,
    id: &str,
) -> Result<ChangeProposal, ApiError> {
    let pr_number = parse_change_id(id)?;
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let pr = gitea.pull_request(pr_number).await?;
    if !pr.head_branch.starts_with("portal/") {
        return Err(ApiError::NotFound(format!(
            "change proposal '{id}' not found in project '{project}'"
        )));
    }

    let data = load_manifest_data(gitea, &pr, project)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "change proposal '{id}' not found in project '{project}'"
            ))
        })?;

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let redacted_fields = redact(plan.fields.clone());
    let author = human_author(gitea, &pr).await;
    let files = changed_files(gitea, &pr).await?;
    Ok(ChangeProposal {
        author,
        file_count: Some(files.len()),
        files: Some(files),
        ..build_proposal(&pr, project, &data, plan, Some(redacted_fields))
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/changes/{id}/approve",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Change proposal ID (chg- + 8 hex digits)"),
    ),
    request_body(
        content = Option<ApproveBody>,
        description = "Optional approval confirmation for red-lane changes",
        content_type = "application/json"
    ),
    responses(
        (status = 202, description = "Change proposal approved and deploying", body = Change),
        (status = 400, description = "Bad request or missing red-lane confirmation", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden (missing role or self-approval)", body = ProblemDetails),
        (status = 404, description = "Change proposal not found", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn approve_change(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let approve_body: Option<ApproveBody> = if body.is_empty() {
        None
    } else {
        Some(
            serde_json::from_slice(&body)
                .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?,
        )
    };
    let confirm = approve_body.as_ref().and_then(|b| b.confirm.as_deref());
    let change = approve_change_for(
        &state,
        &user.0.identity,
        &project,
        &id,
        confirm,
        ApprovedBy::Person,
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// Who presses approve: a person at the Portal's button, or an operation of the registry, which an
/// agent calls on a person's behalf (AG-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovedBy {
    Person,
    Operation,
}

/// Whether a binding covering the project grants `approve` and `delete` on the change's kind: an
/// administrator of that kind, who may approve their own change (PF-58). The bootstrap group is
/// not a binding and does not count.
fn administers(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    data: &ManifestData,
) -> bool {
    let effective = crate::permissions::for_request(state, identity, project);
    if effective.bootstrap {
        return false;
    }
    let target = data
        .head_envelope
        .as_ref()
        .or(data.base_envelope.as_ref())
        .and_then(|envelope| serde_json::to_value(envelope).ok());
    [jc_core::kinds::Verb::Approve, jc_core::kinds::Verb::Delete]
        .into_iter()
        .all(|verb| effective.check(&data.kind, verb, target.as_ref()).is_ok())
}

/// Every file the merge request changes, with the kind, the operation and the lane each one
/// carries (T-0861).
///
/// The same walk `approve_every_file` makes, over the same reads: a page that summarised the
/// merge request differently from the checks would be a page an approver cannot trust. A file
/// whose kind this platform does not serve is still listed — the approval refuses it, and an
/// approver who cannot see it cannot understand the refusal.
/// ponytail: one `get_file` per changed file, as the approval does; the tree diff is the
/// upgrade for both at once.
async fn changed_files(gitea: &GiteaClient, pr: &PullRequest) -> Result<Vec<ChangeFile>, ApiError> {
    let mut listed = Vec::new();
    for file in gitea.pull_request_files(pr.number).await? {
        let git_ref = if file.deleted {
            &pr.base_branch
        } else {
            &pr.head_branch
        };
        let content = gitea
            .get_file(&file.path, git_ref)
            .await?
            .map(|found| found.content)
            .unwrap_or_default();
        let envelope = serde_yaml_ng::from_str::<ResourceEnvelope>(&content).ok();
        let kind = envelope
            .as_ref()
            .map(|envelope| envelope.kind.clone())
            .or_else(|| crate::api::import::native_kind(&file.path).map(str::to_owned))
            .unwrap_or_default();
        let operation = match (file.deleted, file.added) {
            (true, _) => Operation::Delete,
            (_, true) => Operation::Create,
            _ => Operation::Update,
        };
        let lane = match (&envelope, operation) {
            // A delete is red whatever it removes, which is what the approval decides too.
            (_, Operation::Delete) => Lane::Red,
            (Some(envelope), _) => {
                change::classify(&envelope.kind, Operation::Create, &envelope.spec)
            }
            // A native file beside a manifest carries no spec to classify; the manifest it
            // belongs to is in the same merge request and carries the lane.
            (None, _) => Lane::Green,
        };
        listed.push(ChangeFile {
            path: file.path,
            kind,
            operation,
            lane,
        });
    }
    Ok(listed)
}

/// The approval checks of every file the merge request changes, and the strictest lane among
/// them (T-0832). The headline manifest is among them and passes the same checks twice, which
/// costs nothing and keeps this loop free of a special case.
/// ponytail: one `get_file` per changed file; a bundle is a handful, the tree diff is the upgrade.
async fn approve_every_file(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    gitea: &GiteaClient,
    pr: &PullRequest,
) -> Result<Lane, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    let mut lane = Lane::Green;
    for file in gitea.pull_request_files(pr.number).await? {
        let git_ref = if file.deleted {
            &pr.base_branch
        } else {
            &pr.head_branch
        };
        let content = gitea
            .get_file(&file.path, git_ref)
            .await?
            .ok_or_else(|| {
                ApiError::Internal(format!(
                    "'{}' is in change proposal {} but not on {git_ref}",
                    file.path, pr.number
                ))
            })?
            .content;
        let Ok(envelope) = serde_yaml_ng::from_str::<ResourceEnvelope>(&content) else {
            let kind = crate::api::import::native_kind(&file.path).ok_or_else(|| {
                ApiError::Denied(format!(
                    "'{}' belongs to no kind this platform serves, so no role grants approving \
                     it (PF-50)",
                    file.path
                ))
            })?;
            effective.check(kind, jc_core::kinds::Verb::Approve, None)?;
            if file.deleted {
                effective.check(kind, jc_core::kinds::Verb::Delete, None)?;
            }
            continue;
        };
        let manifest =
            serde_json::to_value(&envelope).map_err(|e| ApiError::Internal(e.to_string()))?;
        effective.check(
            &envelope.kind,
            jc_core::kinds::Verb::Approve,
            Some(&manifest),
        )?;
        let access = matches!(
            envelope.kind.as_str(),
            "Role" | "RoleBinding" | "ServiceAccount"
        );
        if file.deleted {
            // `delete` is its own verb on every kind, not only the access ones: a cascade that
            // removes a whole project is held to `approve` and `delete` on each kind it takes
            // with it, so nobody approves away what their role never let them delete (PF-77).
            effective.check(&envelope.kind, jc_core::kinds::Verb::Delete, None)?;
            lane = Lane::Red;
            continue;
        }
        if access {
            crate::permissions::within_own_rights(state, identity, &manifest, "approver")?;
        }
        lane = crate::api::import::riskiest(
            lane,
            change::classify(&envelope.kind, Operation::Create, &envelope.spec),
        );
    }
    Ok(lane)
}

/// Core approval function factored out for reuse by both the REST route and the operations registry.
pub async fn approve_change_for(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    id: &str,
    confirm: Option<&str>,
    by: ApprovedBy,
) -> Result<Change, ApiError> {
    may_approve_anything(state, identity, project)?;
    let pr_number = parse_change_id(id)?;
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let pr = gitea.pull_request(pr_number).await?;
    let data = load_manifest_data(gitea, &pr, project)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "change proposal '{id}' not found in project '{project}'"
            ))
        })?;
    may_approve(state, identity, project, &data)?;
    // Access changes hold the approver to what they grant, and a removal to `delete` (PF-52).
    if matches!(
        data.kind.as_str(),
        "Role" | "RoleBinding" | "ServiceAccount"
    ) {
        match (data.operation, &data.head_envelope) {
            (Operation::Delete, _) => crate::permissions::for_request(state, identity, project)
                .check(&data.kind, jc_core::kinds::Verb::Delete, None)?,
            (_, Some(head)) => {
                let manifest =
                    serde_json::to_value(head).map_err(|e| ApiError::Internal(e.to_string()))?;
                crate::permissions::within_own_rights(state, identity, &manifest, "approver")?;
            }
            (_, None) => {}
        }
    }

    // Every file of the merge request, not only the headline (T-0832, MF-21, CC-63): each
    // manifest needs approve on its kind, the PF-52 hold when it grants access, and its own
    // lane; a native file approves under the kind its directory names.
    let bundle_lane = approve_every_file(state, identity, project, gitea, &pr).await?;

    let author = human_author(gitea, &pr).await;
    let is_author = match (&author.email, &identity.email) {
        (Some(pr_email), Some(user_email)) if !pr_email.trim().is_empty() => {
            pr_email.eq_ignore_ascii_case(user_email.trim())
        }
        _ => {
            let user_name = identity.name.as_deref().unwrap_or(&identity.username);
            author.name.eq_ignore_ascii_case(user_name)
                || author.name.eq_ignore_ascii_case(&identity.username)
        }
    };

    // An author approves their own change only at the button and only as an administrator of its
    // kind (PF-58); an agent never does (AG-11).
    if is_author && !(by == ApprovedBy::Person && administers(state, identity, project, &data)) {
        return Err(ApiError::SelfApproval(
            "proposal author cannot approve their own change (AG-11); an administrator of its kind may, in the Portal (PF-58)".to_string(),
        ));
    }

    let lane = if data.operation == Operation::Delete {
        Lane::Red
    } else if let Some(ref env) = data.head_envelope {
        change::classify(&env.kind, data.operation, &env.spec)
    } else {
        Lane::Yellow
    };
    let lane = crate::api::import::riskiest(lane, bundle_lane);

    if lane == Lane::Red && confirm != Some(&data.name) {
        return Err(ApiError::BadRequest(format!(
            "red lane change requires confirm to be '{}' (CC-19, CC-39)",
            data.name
        )));
    }

    // The Portal's forge token authored the pull request, and Gitea refuses a review from the
    // author (422 "approve your own pull is not allowed"), so the approval is not a forge review:
    // the Portal checked the binding (PF-50) and the author (CC-34) above, and the merge commit
    // records who approved.
    let approver = identity.email.as_deref().unwrap_or(&identity.username);
    let merge_msg = if is_author {
        format!(
            "Merge change proposal {id}: {}\n\nApproved in the Portal by {approver}, its author, as an administrator of {} (PF-58)",
            pr.title, data.kind
        )
    } else {
        format!(
            "Merge change proposal {id}: {}\n\nApproved in the Portal by {approver}",
            pr.title
        )
    };
    // Gitea checks a fresh pull's mergeability in the background and answers 405 "Please try
    // again later" until it has; an approval that follows the proposal within seconds (the
    // demo's, a script's) waits it out instead of failing.
    let mut attempt = 0;
    loop {
        match gitea.merge(pr_number, MergeStyle::Squash, &merge_msg).await {
            Err(GitError::Api { status: 405, .. }) if attempt < 15 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            other => break other?,
        }
    }

    if let Some(syncer) = state.syncer.as_ref() {
        let syncer = syncer.clone();
        tokio::spawn(async move {
            if let Err(err) = syncer.sync_once().await {
                tracing::warn!(error = %err, "sync after change approval failed");
            }
        });
    }

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let change_meta = ChangeMeta::from_merge_request(pr_number, project);
    let change_status =
        ChangeStatus::new(lane, ChangePhase::Deploying, plan.summary).with_merge_request(pr.url);
    let change = Change::new(change_meta, change_status);

    Ok(change)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/changes/{id}/reject",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Change proposal ID (chg- + 8 hex digits)"),
    ),
    request_body(
        content = Option<ApproveBody>,
        description = "Optional reject payload",
        content_type = "application/json"
    ),
    responses(
        (status = 202, description = "Change proposal rejected", body = Change),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Change proposal not found", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn reject_change(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let body: ApproveBody = if body.is_empty() {
        ApproveBody::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?
    };
    let change = reject_change_for(
        &state,
        &user.0.identity,
        &project,
        &id,
        body.reason.as_deref(),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// Rejects a change proposal as `identity`: the one path behind the REST route and
/// `jc_change_reject` (AG-77). Needs `approve` on the change's kind (PF-50); the merge request is
/// closed with a comment naming who rejected it and why.
pub async fn reject_change_for(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    project: &str,
    id: &str,
    reason: Option<&str>,
) -> Result<Change, ApiError> {
    may_approve_anything(state, identity, project)?;
    let pr_number = parse_change_id(id)?;
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let pr = gitea.pull_request(pr_number).await?;
    let data = load_manifest_data(gitea, &pr, project)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "change proposal '{id}' not found in project '{project}'"
            ))
        })?;
    may_approve(state, identity, project, &data)?;

    // A comment, not a "request changes" review: the forge token is the pull request's author
    // and Gitea refuses the author's verdict on their own pull; the Portal's role check above is
    // the gate (PF-50), the comment says who rejected and why.
    let rejecter = identity.email.as_deref().unwrap_or(&identity.username);
    let comment = match reason.map(str::trim).filter(|reason| !reason.is_empty()) {
        Some(reason) => format!("Change proposal rejected in the Portal by {rejecter}: {reason}"),
        None => format!("Change proposal rejected in the Portal by {rejecter}"),
    };
    gitea
        .review(pr_number, ReviewEvent::Comment, &comment)
        .await?;
    // Closed, not merged, is what the list reads back as Rejected; a comment alone would leave
    // the proposal pending.
    gitea.close_pull_request(pr_number).await?;

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let lane = if data.operation == Operation::Delete {
        Lane::Red
    } else if let Some(ref env) = data.head_envelope {
        change::classify(&env.kind, data.operation, &env.spec)
    } else {
        Lane::Yellow
    };

    let change_meta = ChangeMeta::from_merge_request(pr_number, project);
    let change_status =
        ChangeStatus::new(lane, ChangePhase::Rejected, plan.summary).with_merge_request(pr.url);
    Ok(Change::new(change_meta, change_status))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/changes",
            axum::routing::get(list_changes),
        )
        .route(
            "/projects/{project}/changes/{id}",
            axum::routing::get(get_change),
        )
        .route(
            "/projects/{project}/changes/{id}/approve",
            axum::routing::post(approve_change),
        )
        .route(
            "/projects/{project}/changes/{id}/reject",
            axum::routing::post(reject_change),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use serde_json::json;
    use std::sync::Arc;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::auth::session::{Identity, Session};
    use crate::config::Config;

    fn test_user(email: &str, roles: Vec<&str>) -> CurrentUser {
        CurrentUser(Session {
            identity: Identity {
                subject: "sub-approver".into(),
                username: "jana.kovacova".into(),
                email: Some(email.into()),
                name: Some("Jana Kováčová".into()),
                roles: roles.into_iter().map(String::from).collect(),
                groups: Vec::new(),
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            id_token: "dummy".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    #[test]
    fn test_parse_change_id() {
        assert_eq!(parse_change_id("chg-00000001").unwrap(), 1);
        assert_eq!(parse_change_id("chg-0000002a").unwrap(), 42);
        assert_eq!(parse_change_id("chg-0000019c").unwrap(), 412);

        assert!(parse_change_id("mr-00000001").is_err());
        assert!(parse_change_id("chg-1").is_err());
        assert!(parse_change_id("chg-000000001").is_err());
        assert!(parse_change_id("chg-0000001A").is_err());
        assert!(parse_change_id("chg-0000000z").is_err());
    }

    #[test]
    fn test_parse_branch_name() {
        let b1 = parse_branch_name("portal/create-contextspace-mobility-12345678").unwrap();
        assert_eq!(b1.operation, Operation::Create);
        assert_eq!(b1.kind_lower, "contextspace");
        assert_eq!(b1.resource_name, "mobility");
        assert_eq!(b1.hash, "12345678");

        let b2 = parse_branch_name("portal/update-endpoint-public-air-87654321").unwrap();
        assert_eq!(b2.operation, Operation::Update);
        assert_eq!(b2.kind_lower, "endpoint");
        assert_eq!(b2.resource_name, "public-air");
        assert_eq!(b2.hash, "87654321");

        let b3 = parse_branch_name("portal/delete-pipeline-traffic-stream-abcdef01").unwrap();
        assert_eq!(b3.operation, Operation::Delete);
        assert_eq!(b3.kind_lower, "pipeline");
        assert_eq!(b3.resource_name, "traffic-stream");
        assert_eq!(b3.hash, "abcdef01");

        assert!(parse_branch_name("main").is_none());
        assert!(parse_branch_name("portal/invalid").is_none());
        assert!(parse_branch_name("portal/foo-bar-baz-123").is_none());
        // A retry's nonce (T-0887) is not part of the name.
        let b4 = parse_branch_name("portal/update-pipeline-hel-news-0711bca4_144d2358").unwrap();
        assert_eq!(b4.operation, Operation::Update);
        assert_eq!(b4.kind_lower, "pipeline");
        assert_eq!(b4.resource_name, "hel-news");
        assert_eq!(b4.hash, "0711bca4");
    }

    #[test]
    fn test_redact() {
        let fields = vec![
            FieldChange {
                path: "metadata.name".to_string(),
                from: None,
                to: Some(json!("public-air")),
            },
            FieldChange {
                path: "spec.password".to_string(),
                from: Some(json!("old-pass")),
                to: Some(json!("new-pass")),
            },
            FieldChange {
                path: "spec.auth.apiKey".to_string(),
                from: None,
                to: Some(json!("my-api-key")),
            },
            FieldChange {
                path: "spec.token".to_string(),
                from: Some(json!("deleted-token")),
                to: None,
            },
            FieldChange {
                path: "spec.clientSecret".to_string(),
                from: Some(json!("secret-1")),
                to: Some(json!("secret-2")),
            },
        ];

        let redacted = redact(fields);
        assert_eq!(redacted[0].to, Some(json!("public-air")));
        assert_eq!(redacted[1].from, Some(json!("[REDACTED]")));
        assert_eq!(redacted[1].to, Some(json!("[REDACTED]")));
        assert_eq!(redacted[2].from, None);
        assert_eq!(redacted[2].to, Some(json!("[REDACTED]")));
        assert_eq!(redacted[3].from, Some(json!("[REDACTED]")));
        assert_eq!(redacted[3].to, None);
        assert_eq!(redacted[4].from, Some(json!("[REDACTED]")));
        assert_eq!(redacted[4].to, Some(json!("[REDACTED]")));
    }

    #[tokio::test]
    async fn test_list_changes_without_forge_answers_503() {
        let config = Config::for_tests();
        let state = AppState::new(config, None);
        let user = test_user("user@example.sk", vec!["portal-approver"]);

        let res = list_changes(user, State(state), Path("ovzdusie".to_string())).await;
        match res {
            Err(ApiError::Unavailable(msg)) => assert!(msg.contains("git forge")),
            other => panic!("expected ApiError::Unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_approve_checks_role_and_self_approval() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client = GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap();
        let config = Config::for_tests();
        let state = AppState::new(config, None).with_gitea(Arc::new(client));

        // 1. Missing role -> 403 Forbidden
        let viewer = test_user("viewer@example.sk", vec!["portal-viewer"]);
        let err_role = approve_change(
            viewer,
            State(state.clone()),
            Path(("ovzdusie".into(), "chg-00000001".into())),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err_role, ApiError::Denied(_)));

        // 2. Author self-approval -> 403 SelfApproval
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "number": 1,
                "html_url": "https://gitea.example.sk/pulls/1",
                "state": "open",
                "title": "create ContextSpace mobility",
                "head": { "ref": "portal/create-contextspace-mobility-12345678" },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T09:14:22Z",
                "user": {
                    "login": "approver",
                    "full_name": "Jana Kováčová",
                    "email": "approver@example.sk"
                }
            })))
            .mount(&server)
            .await;

        let manifest_content = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
        let b64 = base64::engine::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            manifest_content.as_bytes(),
        );

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
            .and(query_param("ref", "portal/create-contextspace-mobility-12345678"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-1",
                "content": b64
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/1/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "filename": "projects/ovzdusie/spaces/mobility/space.yaml", "status": "added" }
            ])))
            .mount(&server)
            .await;

        let author_approver = test_user("approver@example.sk", vec!["portal-approver"]);
        let err_self = approve_change(
            author_approver,
            State(state),
            Path(("ovzdusie".into(), "chg-00000001".into())),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err_self, ApiError::SelfApproval(_)));
    }

    #[tokio::test]
    async fn test_approve_red_lane_requires_confirm() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client = GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap();
        let config = Config::for_tests();
        let state = AppState::new(config, None).with_gitea(Arc::new(client));

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "number": 2,
                "html_url": "https://gitea.example.sk/pulls/2",
                "state": "open",
                "title": "delete ContextSpace mobility",
                "head": { "ref": "portal/delete-contextspace-mobility-12345678" },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T09:14:22Z",
                "user": {
                    "login": "other.user",
                    "full_name": "Other User",
                    "email": "other@example.sk"
                }
            })))
            .mount(&server)
            .await;

        let manifest_content = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
        let b64 = base64::engine::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            manifest_content.as_bytes(),
        );

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
            .and(query_param("ref", "main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-1",
                "content": b64
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "filename": "projects/ovzdusie/spaces/mobility/space.yaml", "status": "deleted" }
            ])))
            .mount(&server)
            .await;

        let approver = test_user("approver@example.sk", vec!["portal-approver"]);

        // 1. Missing confirm body -> 400
        let err_no_confirm = approve_change(
            approver.clone(),
            State(state.clone()),
            Path(("ovzdusie".into(), "chg-00000002".into())),
            Bytes::new(),
        )
        .await
        .unwrap_err();
        match err_no_confirm {
            ApiError::BadRequest(msg) => assert!(msg.contains("mobility")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        // 2. Wrong confirm body -> 400
        let wrong_body = Bytes::from(r#"{"confirm":"wrong-name"}"#);
        let err_wrong = approve_change(
            approver.clone(),
            State(state.clone()),
            Path(("ovzdusie".into(), "chg-00000002".into())),
            wrong_body,
        )
        .await
        .unwrap_err();
        match err_wrong {
            ApiError::BadRequest(msg) => assert!(msg.contains("mobility")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        // 3. Correct confirm body -> 202 Accepted. No review is posted: the forge token is the
        //    author and the merge message carries the approver instead.
        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/reviews"))
            .respond_with(ResponseTemplate::new(422))
            .expect(0)
            .mount(&server)
            .await;

        // Gitea's mergeability check is still running for the first merge attempt (405); the
        // approval waits and the second attempt lands.
        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/merge"))
            .respond_with(
                ResponseTemplate::new(405)
                    .set_body_json(json!({"message": "Please try again later"})),
            )
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/merge"))
            .and(wiremock::matchers::body_string_contains(
                "Approved in the Portal by",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        let correct_body = Bytes::from(r#"{"confirm":"mobility"}"#);
        let resp = approve_change(
            approver,
            State(state),
            Path(("ovzdusie".into(), "chg-00000002".into())),
            correct_body,
        )
        .await
        .unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let change: Change = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(change.status.phase, ChangePhase::Deploying);
        assert_eq!(change.status.lane, Lane::Red);
    }
}
