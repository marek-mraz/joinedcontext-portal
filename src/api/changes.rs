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
use crate::git::{GiteaClient, MergeStyle, PullRequest, ReviewEvent};
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
}

impl ChangeProposal {
    pub const KIND: &'static str = "Change";
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
                if let Ok(Some(_)) = gitea.get_file(&cand, &pr.base_branch).await {
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
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<ChangeList>, ApiError> {
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

        let Some(data) = load_manifest_data(gitea, &pr, &project).await? else {
            continue;
        };

        let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
        let proposal = build_proposal(&pr, &project, &data, plan, None);
        proposals.push(proposal);
    }

    proposals.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(Json(ChangeList::new(proposals)))
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
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<ChangeProposal>, ApiError> {
    let pr_number = parse_change_id(&id)?;
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

    let data = load_manifest_data(gitea, &pr, &project)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "change proposal '{id}' not found in project '{project}'"
            ))
        })?;

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let redacted_fields = redact(plan.fields.clone());
    let proposal = build_proposal(&pr, &project, &data, plan, Some(redacted_fields));

    Ok(Json(proposal))
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
    if !user.0.identity.roles.iter().any(|r| r == "portal-approver") {
        return Err(ApiError::Forbidden);
    }

    let pr_number = parse_change_id(&id)?;
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let pr = gitea.pull_request(pr_number).await?;
    let data = load_manifest_data(gitea, &pr, &project)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "change proposal '{id}' not found in project '{project}'"
            ))
        })?;

    let is_author = match (&pr.author_email, &user.0.identity.email) {
        (Some(pr_email), Some(user_email)) if !pr_email.trim().is_empty() => {
            pr_email.eq_ignore_ascii_case(user_email.trim())
        }
        _ => {
            let user_name = user
                .0
                .identity
                .name
                .as_deref()
                .unwrap_or(&user.0.identity.username);
            pr.author_name.eq_ignore_ascii_case(user_name)
                || pr
                    .author_name
                    .eq_ignore_ascii_case(&user.0.identity.username)
        }
    };

    if is_author {
        return Err(ApiError::SelfApproval(
            "proposal author cannot approve their own change (AG-11)".to_string(),
        ));
    }

    let lane = if data.operation == Operation::Delete {
        Lane::Red
    } else if let Some(ref env) = data.head_envelope {
        change::classify(&env.kind, data.operation, &env.spec)
    } else {
        Lane::Yellow
    };

    let approve_body: Option<ApproveBody> = if body.is_empty() {
        None
    } else {
        Some(
            serde_json::from_slice(&body)
                .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?,
        )
    };

    if lane == Lane::Red {
        let confirm_val = approve_body.as_ref().and_then(|b| b.confirm.as_deref());
        if confirm_val != Some(&data.name) {
            return Err(ApiError::BadRequest(format!(
                "red lane change requires confirm to be '{}' (CC-19, CC-39)",
                data.name
            )));
        }
    }

    gitea.review(pr_number, ReviewEvent::Approve, "").await?;
    let merge_msg = format!("Merge change proposal {id}: {}", pr.title);
    gitea
        .merge(pr_number, MergeStyle::Squash, &merge_msg)
        .await?;

    if let Some(syncer) = state.syncer.as_ref() {
        let syncer = syncer.clone();
        tokio::spawn(async move {
            if let Err(err) = syncer.sync_once().await {
                tracing::warn!(error = %err, "sync after change approval failed");
            }
        });
    }

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let change_meta = ChangeMeta::from_merge_request(pr_number, &project);
    let change_status =
        ChangeStatus::new(lane, ChangePhase::Deploying, plan.summary).with_merge_request(pr.url);
    let change = Change::new(change_meta, change_status);

    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
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
    if !user.0.identity.roles.iter().any(|r| r == "portal-approver") {
        return Err(ApiError::Forbidden);
    }

    if !body.is_empty() {
        let _: ApproveBody = serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?;
    }

    let pr_number = parse_change_id(&id)?;
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let pr = gitea.pull_request(pr_number).await?;
    let data = load_manifest_data(gitea, &pr, &project)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "change proposal '{id}' not found in project '{project}'"
            ))
        })?;

    gitea
        .review(
            pr_number,
            ReviewEvent::RequestChanges,
            "Change proposal rejected",
        )
        .await?;

    let plan = plan::diff(data.base_envelope.as_ref(), data.head_envelope.as_ref());
    let lane = if data.operation == Operation::Delete {
        Lane::Red
    } else if let Some(ref env) = data.head_envelope {
        change::classify(&env.kind, data.operation, &env.spec)
    } else {
        Lane::Yellow
    };

    let change_meta = ChangeMeta::from_merge_request(pr_number, &project);
    let change_status =
        ChangeStatus::new(lane, ChangePhase::Rejected, plan.summary).with_merge_request(pr.url);
    let change = Change::new(change_meta, change_status);

    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
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
        assert!(matches!(err_role, ApiError::Forbidden));

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

        // 3. Correct confirm body -> 202 Accepted
        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/reviews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls/2/merge"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
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
