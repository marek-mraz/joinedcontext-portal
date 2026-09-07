use std::hash::{Hash, Hasher};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use crate::api::dry_run::{self, DryRunQuery, DryRunResult};
use crate::auth::CurrentUser;
use crate::change::{self, Change, ChangeMeta, ChangePhase, ChangeStatus, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, FileWrite, GitError};
use crate::plan;
use crate::resource::{self, ResourceEnvelope};
use crate::state::AppState;

/// Field names whose string values are credentials wherever they appear: refused on a write
/// (MF-24), redacted in a plan diff (CC-06) and dropped from an export (MF-17).
pub const SECRET_KEYS: &[&str] = &[
    "password",
    "token",
    "secret",
    "clientSecret",
    "apiKey",
    "client_secret",
    "api_key",
    // A CKAN instance is named by `apiTokenRef`; a pasted `apiToken` is the same mistake as
    // a pasted password and is refused the same way (EP-67).
    "apiToken",
    "api_token",
];

/// Detects string-valued literal secrets in manifest payloads (MF-24).
///
/// Returns the first offending key name if any string-valued key named `password`, `token`,
/// `secret`, `clientSecret` or `apiKey` exists at any depth. Object-valued secret references
/// (such as `secretRef: { name: "..." }`) are permitted.
pub fn find_literal_secret(val: &Value) -> Option<String> {
    match val {
        Value::Object(map) => {
            for (k, v) in map {
                if SECRET_KEYS.contains(&k.as_str()) && v.is_string() {
                    return Some(k.clone());
                }
                if let Some(found) = find_literal_secret(v) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for item in arr {
                if let Some(found) = find_literal_secret(item) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_body_to_value(headers: &HeaderMap, bytes: &[u8]) -> Result<Value, ApiError> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");
    let mime = ct.split(';').next().unwrap_or("").trim();

    if mime.is_empty() || mime == "application/json" || mime == "text/json" {
        serde_json::from_slice(bytes)
            .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))
    } else if mime == "application/yaml" || mime == "application/x-yaml" || mime == "text/yaml" {
        serde_yaml_ng::from_slice(bytes)
            .map_err(|e| ApiError::BadRequest(format!("invalid yaml body: {e}")))
    } else {
        Err(ApiError::UnsupportedMediaType(format!(
            "content type '{mime}' is not supported; expected application/json or application/yaml"
        )))
    }
}

fn parse_patch_to_value(headers: &HeaderMap, bytes: &[u8]) -> Result<Value, ApiError> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let mime = ct.split(';').next().unwrap_or("").trim();

    if mime == "application/merge-patch+json" {
        serde_json::from_slice(bytes)
            .map_err(|e| ApiError::BadRequest(format!("invalid json patch: {e}")))
    } else if mime == "application/apply-patch+yaml" {
        serde_yaml_ng::from_slice(bytes)
            .map_err(|e| ApiError::BadRequest(format!("invalid yaml patch: {e}")))
    } else {
        Err(ApiError::UnsupportedMediaType(format!(
            "content type '{mime}' is not supported for PATCH; expected application/merge-patch+json or application/apply-patch+yaml"
        )))
    }
}

pub(crate) fn branch_name(project: &str, kind: &str, name: &str, operation: Operation) -> String {
    let op_str = match operation {
        Operation::Create => "create",
        Operation::Update => "update",
        Operation::Delete => "delete",
    };
    let kind_lower = kind.to_ascii_lowercase();

    let mut hasher = std::hash::DefaultHasher::new();
    (project, kind, name, operation).hash(&mut hasher);
    let hash_val = hasher.finish();
    let hex = format!("{hash_val:016x}");
    let short = &hex[..8];

    format!("portal/{op_str}-{kind_lower}-{name}-{short}")
}

pub(crate) async fn create_or_reuse_branch(
    gitea: &crate::git::GiteaClient,
    branch: &str,
    default_branch: &str,
) -> Result<(), ApiError> {
    if let Err(err) = gitea.create_branch(branch, default_branch).await {
        match err {
            GitError::Conflict(_) => {
                tracing::info!(branch = %branch, "reusing existing branch for retry");
            }
            other => return Err(other.into()),
        }
    }
    Ok(())
}

pub(crate) fn resolve_repo_path(
    envelope: &ResourceEnvelope,
    kind_info: &resource::KindInfo,
    project: &str,
) -> Result<String, ApiError> {
    let space = envelope
        .metadata
        .labels
        .get("joinedcontext.com/space")
        .map(|s| s.as_str())
        .or(envelope.metadata.namespace.as_deref());

    resource::repository_path(kind_info, project, space, &envelope.metadata.name)
        .map_err(ApiError::BadRequest)
}

pub(crate) fn author_credentials(user: &CurrentUser, project: &str) -> (String, String) {
    let author_name = user
        .0
        .identity
        .name
        .as_deref()
        .unwrap_or(&user.0.identity.username)
        .to_string();
    let fallback_email = format!("{}@{project}.local", user.0.identity.username);
    let author_email = user
        .0
        .identity
        .email
        .as_deref()
        .unwrap_or(&fallback_email)
        .to_string();
    (author_name, author_email)
}

/// Shared mutation engine: validates manifest constraints, plans diffs, and submits
/// merge requests to Git under human authorship (MF-12, CC-03, CC-44, CC-63).
#[allow(clippy::too_many_arguments)]
pub async fn propose(
    user: &CurrentUser,
    state: &AppState,
    project: &str,
    plural: &str,
    path_name: Option<&str>,
    operation: Operation,
    dry_run: bool,
    body_val: Value,
) -> Result<Response, ApiError> {
    // 1. Resolve plural catalogue entry
    let kind_info = resource::by_plural(plural).ok_or_else(|| {
        ApiError::NotFound(format!(
            "plural '{plural}' not found in project '{project}'"
        ))
    })?;

    // 2. Deserialize envelope and validate apiVersion, kind and path name
    let mut envelope: ResourceEnvelope = serde_json::from_value(body_val.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid resource envelope: {e}")))?;

    if envelope.api_version != resource::API_VERSION {
        return Err(ApiError::BadRequest(format!(
            "apiVersion '{}' is not supported (expected '{}')",
            envelope.api_version,
            resource::API_VERSION
        )));
    }

    if envelope.kind != kind_info.kind {
        return Err(ApiError::BadRequest(format!(
            "kind '{}' does not match plural '{}' (expected '{}')",
            envelope.kind, plural, kind_info.kind
        )));
    }

    if let Some(expected_name) = path_name {
        if envelope.metadata.name != expected_name {
            return Err(ApiError::BadRequest(format!(
                "metadata.name '{}' does not match path '{}'",
                envelope.metadata.name, expected_name
            )));
        }
    }

    // 3. Namespace validation and default filling
    match envelope.metadata.namespace.as_deref() {
        None | Some("") => {
            envelope.metadata.namespace = Some(project.to_string());
        }
        Some(ns) if ns == project => {}
        Some(foreign) => {
            return Err(ApiError::BadRequest(format!(
                "metadata.namespace '{foreign}' does not match project '{project}'"
            )));
        }
    }

    // 4. Metadata DNS-1123, status rejection (MF-04) and secret rejection (MF-24)
    resource::validate_meta(&envelope.metadata).map_err(ApiError::BadRequest)?;

    if body_val.get("status").is_some() || envelope.status.is_some() {
        return Err(ApiError::BadRequest(
            "status is computed by the platform and cannot be specified in the manifest (MF-04)"
                .into(),
        ));
    }

    if let Some(secret_key) = find_literal_secret(&body_val) {
        return Err(ApiError::BadRequest(format!(
            "literal secret in field '{secret_key}' is forbidden; use secretRef instead (MF-24)"
        )));
    }

    // 4b. The kind's own parse and invariants (T-0412, CC-08, MF-24). `jcctl apply` would
    //     refuse this manifest on `main`, after an approval; refusing it here turns a broken
    //     repository into a form error that names the field. Kinds without a jc-core type
    //     (PORTAL_ONLY_KINDS) have nothing to check against and pass as before.
    if let Some(checked) = jc_core::registry::validate_yaml(
        kind_info.kind,
        &serde_json::to_string(&envelope).map_err(|e| ApiError::Internal(e.to_string()))?,
    ) {
        checked.map_err(|e| {
            ApiError::BadRequest(format!("spec is not a valid {}: {e}", kind_info.kind))
        })?;
    }

    // 4c. Who may propose this kind here, with this content (T-0526, PF-50): the bindings of
    //     the organization repository, before a Change exists. 403 names the verb or the field.
    crate::permissions::for_request(state, &user.0.identity, project).check(
        kind_info.kind,
        jc_core::kinds::Verb::Propose,
        Some(&body_val),
    )?;

    // 5. Diff against current mirror state
    let current = state
        .mirror
        .get(project, kind_info.kind, &envelope.metadata.name);
    let plan = plan::diff(current.as_ref(), Some(&envelope));

    // 6. Risk-classified approval lane
    let lane = change::classify(kind_info.kind, operation, &envelope.spec);
    // OPS-16: the one series no other component can produce. A rise in red proposals is a
    // change in what people are asking the platform to do.
    crate::telemetry::proposed(lane, kind_info.kind);

    // 7. Dry run short-circuit
    if dry_run {
        return Ok((
            StatusCode::OK,
            Json(DryRunResult {
                valid: true,
                lane,
                plan,
            }),
        )
            .into_response());
    }

    // 8. Commit to Git merge request via Gitea client
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let default_branch = gitea.default_branch().await?;

    let op_str = match operation {
        Operation::Create => "create",
        Operation::Update => "update",
        Operation::Delete => "delete",
    };

    let branch = branch_name(project, kind_info.kind, &envelope.metadata.name, operation);
    create_or_reuse_branch(gitea, &branch, &default_branch).await?;
    let repo_path = resolve_repo_path(&envelope, kind_info, project)?;

    let mut envelope_to_commit = envelope.clone();
    envelope_to_commit.strip_status();
    let yaml_content = serde_yaml_ng::to_string(&envelope_to_commit)
        .map_err(|e| ApiError::Internal(format!("serialize manifest to yaml: {e}")))?;

    // Gitea wants the blob sha of the file being replaced, which only `get_file` knows;
    // `status.observedRevision` is a commit id and would be rejected. Asking the branch (not the
    // mirror) also covers a manifest that exists in Git but has not been mirrored yet.
    let existing_sha = gitea
        .get_file(&repo_path, &branch)
        .await
        .ok()
        .flatten()
        .map(|f| f.sha);

    let (author_name, author_email) = author_credentials(user, project);
    let commit_msg = format!("{op_str} {} {}", kind_info.kind, envelope.metadata.name);

    let file_write = FileWrite {
        path: &repo_path,
        branch: &branch,
        message: &commit_msg,
        content: &yaml_content,
        sha: existing_sha.as_deref(),
        author: Author {
            name: &author_name,
            email: &author_email,
        },
    };

    gitea.put_file(&file_write).await?;

    let pr_title = format!("{op_str} {} {}", kind_info.kind, envelope.metadata.name);
    let pr_body = format!(
        "Proposed {op_str} of {} `{}` in project `{project}` via joinedcontext Portal.",
        kind_info.kind, envelope.metadata.name
    );

    let pr = gitea
        .create_pull_request(&branch, &default_branch, &pr_title, &pr_body)
        .await?;

    // 9. Answer 202 Accepted with Change resource
    let change_meta = ChangeMeta::from_merge_request(pr.number, project);
    let change_status = ChangeStatus::new(lane, ChangePhase::PendingApproval, plan.summary)
        .with_merge_request(pr.url);
    let change = Change::new(change_meta, change_status);

    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/{plural}",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' for dry run"),
    ),
    request_body = ResourceEnvelope,
    responses(
        (status = 202, description = "Change proposal accepted", body = Change),
        (status = 200, description = "Dry run validation result", body = DryRunResult),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 415, description = "Unsupported media type", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn create(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, plural)): Path<(String, String)>,
    Query(dry_run_q): Query<DryRunQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let is_dry = dry_run::is_dry_run(&dry_run_q)?;
    let body_val = parse_body_to_value(&headers, &body)?;
    propose(
        &user,
        &state,
        &project,
        &plural,
        None,
        Operation::Create,
        is_dry,
        body_val,
    )
    .await
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/{project}/{plural}/{name}",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("name" = String, Path, description = "Resource name"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' for dry run"),
    ),
    request_body = ResourceEnvelope,
    responses(
        (status = 202, description = "Change proposal accepted", body = Change),
        (status = 200, description = "Dry run validation result", body = DryRunResult),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 415, description = "Unsupported media type", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn replace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, plural, name)): Path<(String, String, String)>,
    Query(dry_run_q): Query<DryRunQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let is_dry = dry_run::is_dry_run(&dry_run_q)?;
    let body_val = parse_body_to_value(&headers, &body)?;
    propose(
        &user,
        &state,
        &project,
        &plural,
        Some(&name),
        Operation::Update,
        is_dry,
        body_val,
    )
    .await
}

#[utoipa::path(
    patch,
    path = "/api/v1/projects/{project}/{plural}/{name}",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("name" = String, Path, description = "Resource name"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' for dry run"),
    ),
    // Declared by hand: the handler takes the raw `Bytes` because the media type decides how the
    // body is parsed, and utoipa cannot derive a schema from that extractor.
    request_body(
        content = String,
        description = "RFC 7386 merge patch, as JSON or as the YAML apply-patch document",
        content_type = "application/merge-patch+json",
    ),
    responses(
        (status = 202, description = "Change proposal accepted", body = Change),
        (status = 200, description = "Dry run validation result", body = DryRunResult),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 415, description = "Unsupported media type", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn patch(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, plural, name)): Path<(String, String, String)>,
    Query(dry_run_q): Query<DryRunQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let is_dry = dry_run::is_dry_run(&dry_run_q)?;
    let patch_val = parse_patch_to_value(&headers, &body)?;

    if patch_val.get("status").is_some() {
        return Err(ApiError::BadRequest(
            "status is computed by the platform and cannot be specified in the manifest (MF-04)"
                .into(),
        ));
    }

    if let Some(secret_key) = find_literal_secret(&patch_val) {
        return Err(ApiError::BadRequest(format!(
            "literal secret in field '{secret_key}' is forbidden; use secretRef instead (MF-24)"
        )));
    }

    let kind_info = resource::by_plural(&plural).ok_or_else(|| {
        ApiError::NotFound(format!(
            "plural '{plural}' not found in project '{project}'"
        ))
    })?;

    let current = state
        .mirror
        .get(&project, kind_info.kind, &name)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "resource '{name}' not found in project '{project}'"
            ))
        })?;

    let mut curr_to_patch = current;
    curr_to_patch.strip_status();
    let mut desired_val = serde_json::to_value(&curr_to_patch).map_err(|e| {
        ApiError::Internal(format!("failed to serialize current resource to json: {e}"))
    })?;

    plan::merge_patch(&mut desired_val, &patch_val);

    propose(
        &user,
        &state,
        &project,
        &plural,
        Some(&name),
        Operation::Update,
        is_dry,
        desired_val,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session::{Identity, Session};
    use crate::change::Lane;
    use crate::config::Config;
    use crate::resource::ObjectMeta;
    use http_body_util::BodyExt;
    use serde_json::json;

    fn dummy_user() -> CurrentUser {
        CurrentUser(Session {
            identity: Identity {
                subject: "sub-123".into(),
                username: "demo.developer".into(),
                email: Some("demo@example.com".into()),
                name: Some("Demo Developer".into()),
                roles: vec![],
                groups: vec!["portal-approver".into()],
            },
            expires_at: 9_999_999_999,
            issued_at: 1000,
            id_token: "dummy-token".into(),
            access_expires_at: 9_999_999_999,
            refresh_token: None,
        })
    }

    #[test]
    fn detects_literal_secrets_and_permits_secret_refs() {
        assert_eq!(
            find_literal_secret(&json!({ "password": "supersecret" })),
            Some("password".into())
        );
        assert_eq!(
            find_literal_secret(&json!({ "spec": { "token": "ghp_123" } })),
            Some("token".into())
        );
        assert_eq!(
            find_literal_secret(&json!({ "spec": { "nested": { "clientSecret": "plain" } } })),
            Some("clientSecret".into())
        );
        assert_eq!(
            find_literal_secret(&json!({ "apiKey": "12345" })),
            Some("apiKey".into())
        );

        assert_eq!(
            find_literal_secret(&json!({
                "spec": {
                    "password": {
                        "secretRef": { "name": "db-secret", "key": "password" }
                    }
                }
            })),
            None
        );
        assert_eq!(
            find_literal_secret(&json!({
                "spec": {
                    "secretRef": { "name": "vault-key" }
                }
            })),
            None
        );
        assert_eq!(
            find_literal_secret(&json!({
                "metadata": { "name": "public-air" },
                "spec": { "audience": "public" }
            })),
            None
        );
    }

    #[tokio::test]
    async fn propose_dry_run_create_returns_dry_run_result() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();
        let payload = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": "mobility",
                "namespace": "ovzdusie"
            },
            "spec": {
                "isSandbox": true
            }
        });

        let resp = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            payload,
        )
        .await
        .expect("dry run create should succeed");

        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: DryRunResult = serde_json::from_slice(&body_bytes).unwrap();
        assert!(result.valid);
        assert_eq!(result.lane, Lane::Green);
        assert_eq!(result.plan.summary.create, 1);
        assert_eq!(result.plan.summary.update, 0);
    }

    #[tokio::test]
    async fn propose_validation_rejects_wrong_apiversion_or_kind() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();

        let bad_api = json!({
            "apiVersion": "v1",
            "kind": "ContextSpace",
            "metadata": { "name": "mobility" },
            "spec": {}
        });
        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            bad_api,
        )
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("apiVersion")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        let bad_kind = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "Endpoint",
            "metadata": { "name": "mobility" },
            "spec": {}
        });
        let err2 = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            bad_kind,
        )
        .await
        .unwrap_err();
        match err2 {
            ApiError::BadRequest(msg) => assert!(msg.contains("kind 'Endpoint'")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn propose_validation_rejects_foreign_namespace_and_fills_absent() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();

        let foreign_ns = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": "mobility",
                "namespace": "foreign-project"
            },
            "spec": {}
        });
        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            foreign_ns,
        )
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("foreign-project")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        let absent_ns = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": "mobility"
            },
            "spec": {}
        });
        let resp = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            absent_ns,
        )
        .await
        .expect("absent namespace should be filled in as project");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn propose_validation_rejects_status_and_literal_secrets() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();

        let with_status = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mobility" },
            "spec": {},
            "status": { "phase": "Live" }
        });
        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            with_status,
        )
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("status")),
            other => panic!("expected BadRequest, got {other:?}"),
        }

        let with_secret = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mobility" },
            "spec": {
                "auth": { "password": "supersecretpassword" }
            }
        });
        let err2 = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            true,
            with_secret,
        )
        .await
        .unwrap_err();
        match err2 {
            ApiError::BadRequest(msg) => assert!(msg.contains("literal secret")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn propose_rejects_name_mismatch_on_replace() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();

        let payload = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mobility-b" },
            "spec": {}
        });
        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            Some("mobility-a"),
            Operation::Update,
            true,
            payload,
        )
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(msg) => assert!(msg.contains("mobility-b")),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn propose_non_dry_run_without_gitea_answers_503() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();
        let payload = json!({
            "apiVersion": resource::API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mobility" },
            "spec": {}
        });

        let err = propose(
            &user,
            &state,
            "ovzdusie",
            "spaces",
            None,
            Operation::Create,
            false,
            payload,
        )
        .await
        .unwrap_err();

        match err {
            ApiError::Unavailable(msg) => assert!(msg.contains("git forge")),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn parse_patch_content_types() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json".parse().expect("header"),
        );
        let err = parse_patch_to_value(&headers, b"{}").unwrap_err();
        match err {
            ApiError::UnsupportedMediaType(msg) => {
                assert!(msg.contains("application/merge-patch+json"))
            }
            other => panic!("expected UnsupportedMediaType, got {other:?}"),
        }

        let mut headers_valid = HeaderMap::new();
        headers_valid.insert(
            header::CONTENT_TYPE,
            "application/merge-patch+json".parse().expect("header"),
        );
        let val = parse_patch_to_value(&headers_valid, b"{\"spec\":{\"audience\":\"public\"}}")
            .expect("parse valid json patch");
        assert_eq!(val["spec"]["audience"], "public");

        let mut headers_yaml = HeaderMap::new();
        headers_yaml.insert(
            header::CONTENT_TYPE,
            "application/apply-patch+yaml".parse().expect("header"),
        );
        let val_yaml = parse_patch_to_value(&headers_yaml, b"spec:\n  audience: public\n")
            .expect("parse valid yaml patch");
        assert_eq!(val_yaml["spec"]["audience"], "public");
    }

    #[tokio::test]
    async fn patch_merges_with_current_and_plans_diff() {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(ResourceEnvelope {
            api_version: resource::API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: ObjectMeta {
                name: "mobility".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({ "isSandbox": false }),
            status: None,
        });

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/merge-patch+json".parse().expect("header"),
        );
        let patch_body = Bytes::from(r#"{"spec":{"isSandbox":true}}"#);

        let user = dummy_user();
        let resp = patch(
            user,
            State(state),
            Path(("ovzdusie".into(), "spaces".into(), "mobility".into())),
            Query(DryRunQuery {
                dry_run: Some("All".into()),
            }),
            headers,
            patch_body,
        )
        .await
        .expect("patch dry run should succeed");

        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let result: DryRunResult = serde_json::from_slice(&body_bytes).unwrap();
        assert!(result.valid);
        assert_eq!(result.lane, Lane::Green);
        assert_eq!(result.plan.summary.update, 1);
        assert_eq!(result.plan.fields.len(), 1);
        assert_eq!(result.plan.fields[0].path, "spec.isSandbox");
        assert_eq!(result.plan.fields[0].from, Some(json!(false)));
        assert_eq!(result.plan.fields[0].to, Some(json!(true)));
    }
}
