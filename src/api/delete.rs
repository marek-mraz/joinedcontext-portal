use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use crate::api::dry_run::{self, DryRunQuery, DryRunResult};
use crate::api::mutate::{
    author_credentials, branch_name, create_or_reuse_branch, open_change_on, resolve_repo_path,
};
use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::change::{self, Change, ChangeMeta, ChangePhase, ChangeStatus, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::Author;
use crate::plan;
use crate::resource;
use crate::state::AppState;

/// Every file beside the manifest that belongs to the resource it describes (T-0900).
///
/// A resource's own files sit in the manifest's directory and are named after it: either
/// `{name}.something` — `{name}.linkml.yaml`, `{name}.schema.json` — or everything under a
/// `{name}/` directory. A manifest name is DNS-1123 and carries no dot, so nothing else in
/// that directory can begin with `{name}.`.
async fn owned_beside(
    gitea: &crate::git::GiteaClient,
    repo_path: &str,
    name: &str,
    git_ref: &str,
) -> Result<Vec<String>, ApiError> {
    let directory = repo_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let file_prefix = format!("{directory}/{name}.");
    let folder_prefix = format!("{directory}/{name}/");
    Ok(gitea
        .list_tree(git_ref)
        .await?
        .into_iter()
        .filter(|path| {
            path != repo_path
                && (path.starts_with(&file_prefix) || path.starts_with(&folder_prefix))
        })
        .collect())
}

/// Whether a manifest other than the one being deleted still names this file.
///
/// Two DataModels may share one LinkML source: the one that names it keeps it, and the delete
/// removes the manifest alone. Compared by the file name the manifest would carry — the paths
/// in a manifest are relative to it (`./{name}.linkml.yaml`), never repository paths.
fn still_named(state: &AppState, project: &str, kind: &str, name: &str, path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    !state
        .mirror
        .matching(|candidate| {
            let same = candidate.kind == kind
                && candidate.metadata.name == name
                && candidate.metadata.namespace.as_deref() == Some(project);
            !same && names_file(&candidate.spec, file)
        })
        .is_empty()
}

/// Whether any string in the spec ends with this file name.
fn names_file(spec: &Value, file: &str) -> bool {
    match spec {
        Value::String(value) => value.trim_start_matches("./").ends_with(file),
        Value::Object(map) => map.values().any(|child| names_file(child, file)),
        Value::Array(items) => items.iter().any(|child| names_file(child, file)),
        _ => false,
    }
}

/// Traverses a JSON value to detect typed references `{kind, name, namespace?}` (MF-07).
pub fn has_typed_ref(
    val: &Value,
    referrer_ns: &str,
    target_ns: &str,
    target_kind: &str,
    target_name: &str,
) -> bool {
    match val {
        Value::Object(map) => {
            let matches_kind = map.get("kind").and_then(Value::as_str) == Some(target_kind);
            let matches_name = map.get("name").and_then(Value::as_str) == Some(target_name);
            if matches_kind && matches_name {
                let ref_ns = map
                    .get("namespace")
                    .and_then(Value::as_str)
                    .unwrap_or(referrer_ns);
                if ref_ns == target_ns {
                    return true;
                }
            }
            map.values()
                .any(|child| has_typed_ref(child, referrer_ns, target_ns, target_kind, target_name))
        }
        Value::Array(arr) => arr
            .iter()
            .any(|child| has_typed_ref(child, referrer_ns, target_ns, target_kind, target_name)),
        _ => false,
    }
}

/// A resource that still references the one being deleted (MF-07).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Reference {
    pub kind: String,
    pub name: String,
}

/// What a deletion comes to: its plan on a dry run, the Change it opened, or the resources that
/// block it.
#[derive(Debug)]
pub enum DeleteOutcome {
    DryRun(DryRunResult),
    Proposed(Change),
    /// Removed on a workspace's branch; no Change until it is brought back (API/01 §22).
    Workspace(crate::api::mutate::WorkspaceCommit),
    /// The references of the caller's project by name, and how many other projects hold one.
    Referenced {
        here: Vec<Reference>,
        elsewhere: usize,
    },
}

impl DeleteOutcome {
    /// The refusal a blocked deletion answers on REST: the references of this project named, the
    /// others only counted, since another project's resources are not the caller's to see.
    pub fn conflict_message(here: &[Reference], elsewhere: usize) -> String {
        let count = here.len() + elsewhere;
        let mut msg = if count == 1 {
            "1 dependent resource blocks deletion".to_string()
        } else {
            format!("{count} dependent resources block deletion")
        };
        if !here.is_empty() {
            let names: Vec<String> = here
                .iter()
                .map(|r| format!("{} {}", r.kind, r.name))
                .collect();
            msg.push_str(&format!(": {}", names.join(", ")));
        }
        if elsewhere > 0 && !here.is_empty() {
            msg.push_str(&format!(" and {elsewhere} in other projects"));
        }
        msg
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/{plural}/{name}",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("name" = String, Path, description = "Resource name"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' for dry run"),
    ),
    responses(
        (status = 202, description = "Change proposal accepted", body = Change),
        (status = 200, description = "Dry run validation result", body = DryRunResult),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 409, description = "Conflict - blocking dependents", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn delete_resource(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, plural, name)): Path<(String, String, String)>,
    Query(dry_run_q): Query<DryRunQuery>,
) -> Result<Response, ApiError> {
    let is_dry = dry_run::is_dry_run(&dry_run_q)?;
    match delete_with_identity(
        &user.0.identity,
        &state,
        &project,
        &plural,
        &name,
        is_dry,
        dry_run_q.workspace.as_deref(),
    )
    .await?
    {
        DeleteOutcome::DryRun(result) => Ok((StatusCode::OK, Json(result)).into_response()),
        DeleteOutcome::Workspace(commit) => Ok((StatusCode::OK, Json(commit)).into_response()),
        DeleteOutcome::Proposed(change) => Ok((StatusCode::ACCEPTED, Json(change)).into_response()),
        DeleteOutcome::Referenced { here, elsewhere } => Err(ApiError::Conflict(
            DeleteOutcome::conflict_message(&here, elsewhere),
        )),
    }
}

/// Proposes the deletion of one resource as `identity`: the one path behind the REST route and
/// `jc_resource_delete` (AG-77, ADR-N-021). Needs `delete` on the kind (PF-50), refuses while
/// another resource references the target (MF-07) and opens a Red-lane Change (CC-19).
pub async fn delete_with_identity(
    identity: &Identity,
    state: &AppState,
    project: &str,
    plural: &str,
    name: &str,
    dry_run: bool,
    workspace: Option<&str>,
) -> Result<DeleteOutcome, ApiError> {
    let not_found = || {
        ApiError::NotFound(format!(
            "resource '{name}' not found in project '{project}'"
        ))
    };

    // 1. Resolve plural catalogue entry and resource from mirror
    let kind_info = resource::by_plural(plural).ok_or_else(not_found)?;
    // Inside a workspace the resource and what references it are read on its branch (CC-76).
    let mirror = match workspace {
        Some(workspace) => {
            std::sync::Arc::new(crate::ops::workspaces::mirror_of(state, workspace, project).await?)
        }
        None => state.mirror.clone(),
    };
    let envelope = mirror
        .get(project, kind_info.kind, name)
        .ok_or_else(not_found)?;

    // 1b. Deletion needs `delete` in a binding that covers the project (T-0526, PF-50).
    let target = serde_json::to_value(&envelope).map_err(|e| ApiError::Internal(e.to_string()))?;
    crate::permissions::for_request(state, identity, project).check(
        kind_info.kind,
        jc_core::kinds::Verb::Delete,
        Some(&target),
    )?;

    // 2. Every resource in the mirror that still references the target (MF-07, R20)
    let dependents = mirror.matching(|candidate| {
        let candidate_ns = candidate.metadata.namespace.as_deref().unwrap_or_default();
        let is_victim = candidate.kind == kind_info.kind
            && candidate.metadata.name == name
            && candidate_ns == project;
        !is_victim && has_typed_ref(&candidate.spec, candidate_ns, project, kind_info.kind, name)
    });
    if !dependents.is_empty() {
        let (local, foreign): (Vec<_>, Vec<_>) = dependents
            .into_iter()
            .partition(|candidate| candidate.metadata.namespace.as_deref() == Some(project));
        let mut here: Vec<Reference> = local
            .into_iter()
            .map(|candidate| Reference {
                kind: candidate.kind,
                name: candidate.metadata.name,
            })
            .collect();
        here.sort_by(|a, b| (&a.kind, &a.name).cmp(&(&b.kind, &b.name)));
        return Ok(DeleteOutcome::Referenced {
            here,
            elsewhere: foreign.len(),
        });
    }

    // 3. Risk-classified approval lane: Red (CC-19, CC-39, CC-63)
    let lane = change::classify(kind_info.kind, Operation::Delete, &envelope.spec);

    // 4. Compute diff against None (deletion plan)
    let plan = plan::diff(Some(&envelope), None);

    if dry_run {
        return Ok(DeleteOutcome::DryRun(DryRunResult {
            valid: true,
            // A removal takes the stream away rather than restarting it, so the notice that
            // warns about a restart would be the wrong thing to say here (T-1056).
            restarts_stream: false,
            lane,
            plan,
            probe: None,
            verdict: None,
            findings: Vec::new(),
        }));
    }

    // 5. Commit deletion to Git merge request via Gitea client
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let default_branch = gitea.default_branch().await?;
    let repo_path = resolve_repo_path(&envelope, kind_info, project)?;

    let (branch, read_from) = match workspace {
        Some(workspace) => {
            let open = state
                .workspaces
                .live(workspace)
                .await
                .map_err(|err| ApiError::NotFound(err.to_string()))?;
            crate::api::mutate::within_workspace(&open, project, kind_info.kind, &envelope)?;
            if !crate::ops::workspaces::owns(&open, identity) {
                return Err(ApiError::Denied(format!(
                    "workspace '{workspace}' belongs to {}; only its owner writes into it",
                    open.owner
                )));
            }
            (open.branch(), open.branch())
        }
        None => {
            let branch = branch_name(project, kind_info.kind, name, Operation::Delete);
            // One open change per resource (CC-34, T-0883): the pending removal is decided first.
            if let Some(pending) = open_change_on(gitea, &branch, project).await? {
                return Err(ApiError::Conflict(format!(
                    "a change for {} '{name}' is already open: {}; approve or reject it first",
                    kind_info.kind, pending.name
                )));
            }
            (
                create_or_reuse_branch(gitea, &branch, &default_branch).await?,
                default_branch.clone(),
            )
        }
    };
    let existing = gitea
        .get_file(&repo_path, &read_from)
        .await?
        .ok_or_else(not_found)?;

    // The manifest is not the whole resource: a DataModel owns its LinkML source and whatever
    // was rendered from it, and an App, a Pipeline and a Dashboard own native files the same
    // way. They go in the same change, or the repository keeps an orphan a later resource of
    // the same name would inherit (T-0900, MF-07, CC-08). Read once the removal is going
    // ahead: a refusal must not cost a tree listing.
    let mut removals = vec![(repo_path.clone(), existing.sha.clone())];
    for path in owned_beside(gitea, &repo_path, name, &read_from).await? {
        if still_named(state, project, kind_info.kind, name, &path) {
            continue;
        }
        if let Some(file) = gitea.get_file(&path, &read_from).await? {
            removals.push((path, file.sha));
        }
    }

    let (author_name, author_email) = author_credentials(identity, project);
    let commit_msg = format!("delete {} {name}", kind_info.kind);

    // One commit for every file of the resource: a removal that lands in pieces can be
    // approved in pieces, which is how an orphan survives a merged delete.
    gitea
        .change_files(
            &branch,
            &commit_msg,
            Author {
                name: &author_name,
                email: &author_email,
            },
            &[],
            &removals,
        )
        .await?;

    if let Some(workspace) = workspace {
        return Ok(DeleteOutcome::Workspace(
            crate::api::mutate::WorkspaceCommit {
                workspace: workspace.to_owned(),
                branch,
                path: repo_path,
                lane,
            },
        ));
    }

    let pr_title = format!("delete {} {name}", kind_info.kind);
    let pr_body = format!(
        "Proposed delete of {} `{name}` in project `{project}` via joinedcontext Portal.",
        kind_info.kind
    );

    let pr = gitea
        .create_pull_request(&branch, &default_branch, &pr_title, &pr_body)
        .await?;

    let change_meta = ChangeMeta::from_merge_request(pr.number, project);
    let change_status = ChangeStatus::new(lane, ChangePhase::PendingApproval, plan.summary)
        .with_merge_request(pr.url);
    Ok(DeleteOutcome::Proposed(Change::new(
        change_meta,
        change_status,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::extract::State;
    use http_body_util::BodyExt;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::auth::session::{Identity, Session};
    use crate::change::Lane;
    use crate::config::Config;
    use crate::git::GiteaClient;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

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
    fn has_typed_ref_scenarios() {
        let same_ns_ref = json!({
            "target": {
                "kind": "ContextSpace",
                "name": "mobility"
            }
        });
        assert!(has_typed_ref(
            &same_ns_ref,
            "ovzdusie",
            "ovzdusie",
            "ContextSpace",
            "mobility"
        ));

        let explicit_matching_ns = json!({
            "ref": {
                "kind": "ContextSpace",
                "name": "mobility",
                "namespace": "ovzdusie"
            }
        });
        assert!(has_typed_ref(
            &explicit_matching_ns,
            "foreign-proj",
            "ovzdusie",
            "ContextSpace",
            "mobility"
        ));

        let explicit_foreign_ns = json!({
            "ref": {
                "kind": "ContextSpace",
                "name": "mobility",
                "namespace": "other-ns"
            }
        });
        assert!(!has_typed_ref(
            &explicit_foreign_ns,
            "ovzdusie",
            "ovzdusie",
            "ContextSpace",
            "mobility"
        ));

        let foreign_referrer_implicit_ns = json!({
            "ref": {
                "kind": "ContextSpace",
                "name": "mobility"
            }
        });
        assert!(!has_typed_ref(
            &foreign_referrer_implicit_ns,
            "foreign-proj",
            "ovzdusie",
            "ContextSpace",
            "mobility"
        ));

        let nested_array = json!({
            "pipelines": [
                {
                    "endpoints": [
                        { "kind": "Endpoint", "name": "air-sensor" }
                    ]
                }
            ]
        });
        assert!(has_typed_ref(
            &nested_array,
            "ovzdusie",
            "ovzdusie",
            "Endpoint",
            "air-sensor"
        ));

        let mismatched_kind = json!({ "kind": "Pipeline", "name": "mobility" });
        assert!(!has_typed_ref(
            &mismatched_kind,
            "ovzdusie",
            "ovzdusie",
            "ContextSpace",
            "mobility"
        ));

        let mismatched_name = json!({ "kind": "ContextSpace", "name": "traffic" });
        assert!(!has_typed_ref(
            &mismatched_name,
            "ovzdusie",
            "ovzdusie",
            "ContextSpace",
            "mobility"
        ));

        let non_string_type = json!({ "kind": 123, "name": "mobility" });
        assert!(!has_typed_ref(
            &non_string_type,
            "ovzdusie",
            "ovzdusie",
            "ContextSpace",
            "mobility"
        ));
    }

    #[tokio::test]
    async fn delete_unknown_plural_and_missing_name_returns_404() {
        let state = AppState::new(Config::for_tests(), None);
        let user = dummy_user();

        let err_plural = delete_resource(
            user.clone(),
            State(state.clone()),
            Path(("ovzdusie".into(), "unknownplural".into(), "mobility".into())),
            Query(DryRunQuery::default()),
        )
        .await
        .unwrap_err();
        match err_plural {
            ApiError::NotFound(msg) => assert!(msg.contains("mobility")),
            other => panic!("expected NotFound, got {other:?}"),
        }

        let err_name = delete_resource(
            user,
            State(state),
            Path(("ovzdusie".into(), "spaces".into(), "nonexistent".into())),
            Query(DryRunQuery::default()),
        )
        .await
        .unwrap_err();
        match err_name {
            ApiError::NotFound(msg) => assert!(msg.contains("nonexistent")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_victim_itself_not_counted_as_dependent() {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: ObjectMeta {
                name: "mobility".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({
                "selfRef": {
                    "kind": "ContextSpace",
                    "name": "mobility"
                }
            }),
            status: None,
        });

        let user = dummy_user();
        let resp = delete_resource(
            user,
            State(state),
            Path(("ovzdusie".into(), "spaces".into(), "mobility".into())),
            Query(DryRunQuery {
                workspace: None,
                dry_run: Some("All".into()),
            }),
        )
        .await
        .expect("dry run deletion should succeed even when victim self-references in spec");

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let res: DryRunResult = serde_json::from_slice(&bytes).unwrap();
        assert!(res.valid);
        assert_eq!(res.lane, Lane::Red);
        assert_eq!(res.plan.summary.delete, 1);
    }

    #[tokio::test]
    async fn delete_blocked_by_dependents_returns_409_naming_only_count() {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: ObjectMeta {
                name: "mobility".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({ "isSandbox": false }),
            status: None,
        });
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: ObjectMeta {
                name: "live-traffic".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({
                "spaceRef": {
                    "kind": "ContextSpace",
                    "name": "mobility"
                }
            }),
            status: None,
        });

        let user = dummy_user();
        let err1 = delete_resource(
            user.clone(),
            State(state.clone()),
            Path(("ovzdusie".into(), "spaces".into(), "mobility".into())),
            Query(DryRunQuery::default()),
        )
        .await
        .unwrap_err();

        match err1 {
            ApiError::Conflict(msg) => {
                assert_eq!(
                    msg,
                    "1 dependent resource blocks deletion: Endpoint live-traffic"
                );
            }
            other => panic!("expected Conflict, got {other:?}"),
        }

        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Pipeline".into(),
            metadata: ObjectMeta {
                name: "traffic-stream".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({
                "space": {
                    "kind": "ContextSpace",
                    "name": "mobility"
                }
            }),
            status: None,
        });

        let err2 = delete_resource(
            user,
            State(state),
            Path(("ovzdusie".into(), "spaces".into(), "mobility".into())),
            Query(DryRunQuery::default()),
        )
        .await
        .unwrap_err();

        match err2 {
            ApiError::Conflict(msg) => {
                assert_eq!(
                    msg,
                    "2 dependent resources block deletion: Endpoint live-traffic, Pipeline traffic-stream"
                );
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_without_forge_answers_503() {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: ObjectMeta {
                name: "mobility".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({}),
            status: None,
        });

        let user = dummy_user();
        let err = delete_resource(
            user,
            State(state),
            Path(("ovzdusie".into(), "spaces".into(), "mobility".into())),
            Query(DryRunQuery::default()),
        )
        .await
        .unwrap_err();

        match err {
            ApiError::Unavailable(msg) => assert!(msg.contains("git forge")),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_with_forge_returns_202_accepted() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client = GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap();

        let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: ObjectMeta {
                name: "mobility".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({ "isSandbox": true }),
            status: None,
        });

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "default_branch": "main"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
            .and(query_param("ref", "main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "sha-space-123",
                "content": "YXBpVmVyc2lvbjogeW91"
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/branches"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(wiremock::matchers::path_regex(
                r"^/api/v1/repos/test-owner/test-repo/git/trees/.*$",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "tree": [{ "path": "projects/ovzdusie/spaces/mobility/space.yaml", "type": "blob" }],
                "truncated": false
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/contents"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "commit": { "sha": "commit-sha-deleted" }
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "number": 55,
                "html_url": "https://gitea.example.sk/pulls/55",
                "state": "open",
                "mergeable": true,
                "merged": false
            })))
            .mount(&server)
            .await;

        let user = dummy_user();
        let resp = delete_resource(
            user,
            State(state),
            Path(("ovzdusie".into(), "spaces".into(), "mobility".into())),
            Query(DryRunQuery::default()),
        )
        .await
        .expect("delete should succeed");

        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let change: Change = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(change.metadata.name, "chg-00000037");
        assert_eq!(change.metadata.namespace, "ovzdusie");
        assert_eq!(change.status.lane, Lane::Red);
        assert_eq!(change.status.phase, ChangePhase::PendingApproval);
        assert_eq!(change.status.plan.create, 0);
        assert_eq!(change.status.plan.update, 0);
        assert_eq!(change.status.plan.delete, 1);
        assert_eq!(
            change.status.merge_request.as_deref(),
            Some("https://gitea.example.sk/pulls/55")
        );
    }

    #[tokio::test]
    async fn delete_file_missing_in_git_returns_404() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client = GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap();

        let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: ObjectMeta {
                name: "mobility".into(),
                namespace: Some("ovzdusie".into()),
                ..Default::default()
            },
            spec: json!({}),
            status: None,
        });

        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "default_branch": "main"
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
            .and(query_param("ref", "main"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "message": "not found"
            })))
            .mount(&server)
            .await;

        let user = dummy_user();
        let err = delete_resource(
            user,
            State(state),
            Path(("ovzdusie".into(), "spaces".into(), "mobility".into())),
            Query(DryRunQuery::default()),
        )
        .await
        .unwrap_err();

        match err {
            ApiError::NotFound(msg) => assert!(msg.contains("mobility")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
