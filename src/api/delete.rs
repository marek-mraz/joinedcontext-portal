use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use crate::api::dry_run::{self, DryRunQuery, DryRunResult};
use crate::api::mutate::{
    author_credentials, branch_name, create_or_reuse_branch, resolve_repo_path,
};
use crate::auth::CurrentUser;
use crate::change::{self, Change, ChangeMeta, ChangePhase, ChangeStatus, Operation};
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, FileDelete};
use crate::plan;
use crate::resource;
use crate::state::AppState;

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

    let not_found = || {
        ApiError::NotFound(format!(
            "resource '{name}' not found in project '{project}'"
        ))
    };

    // 1. Resolve plural catalogue entry and resource from mirror
    let kind_info = resource::by_plural(&plural).ok_or_else(not_found)?;
    let envelope = state
        .mirror
        .get(&project, kind_info.kind, &name)
        .ok_or_else(not_found)?;

    // 2. Scan every resource in the mirror for blocking dependents (MF-07, R20)
    let blocking_count = state.mirror.count_matching(|candidate| {
        let candidate_ns = candidate.metadata.namespace.as_deref().unwrap_or_default();
        let is_victim = candidate.kind == kind_info.kind
            && candidate.metadata.name == name
            && candidate_ns == project;
        if is_victim {
            return false;
        }
        has_typed_ref(
            &candidate.spec,
            candidate_ns,
            &project,
            kind_info.kind,
            &name,
        )
    });

    if blocking_count > 0 {
        let msg = if blocking_count == 1 {
            "1 dependent resource blocks deletion".to_string()
        } else {
            format!("{blocking_count} dependent resources block deletion")
        };
        return Err(ApiError::Conflict(msg));
    }

    // 3. Risk-classified approval lane: Red (CC-19, CC-39, CC-63)
    let lane = change::classify(kind_info.kind, Operation::Delete, &envelope.spec);

    // 4. Compute diff against None (deletion plan)
    let plan = plan::diff(Some(&envelope), None);

    if is_dry {
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

    // 5. Commit deletion to Git merge request via Gitea client
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;

    let default_branch = gitea.default_branch().await?;
    let repo_path = resolve_repo_path(&envelope, kind_info, &project)?;

    let existing = gitea
        .get_file(&repo_path, &default_branch)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "resource '{name}' not found in project '{project}'"
            ))
        })?;

    let branch = branch_name(&project, kind_info.kind, &name, Operation::Delete);
    create_or_reuse_branch(gitea, &branch, &default_branch).await?;

    let (author_name, author_email) = author_credentials(&user, &project);
    let commit_msg = format!("delete {} {name}", kind_info.kind);

    let file_del = FileDelete {
        path: &repo_path,
        branch: &branch,
        message: &commit_msg,
        sha: &existing.sha,
        author: Author {
            name: &author_name,
            email: &author_email,
        },
    };

    gitea.delete_file(&file_del).await?;

    let pr_title = format!("delete {} {name}", kind_info.kind);
    let pr_body = format!(
        "Proposed delete of {} `{name}` in project `{project}` via joinedcontext Portal.",
        kind_info.kind
    );

    let pr = gitea
        .create_pull_request(&branch, &default_branch, &pr_title, &pr_body)
        .await?;

    let change_meta = ChangeMeta::from_merge_request(pr.number, &project);
    let change_status = ChangeStatus::new(lane, ChangePhase::PendingApproval, plan.summary)
        .with_merge_request(pr.url);
    let change = Change::new(change_meta, change_status);

    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
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
                assert_eq!(msg, "1 dependent resource blocks deletion");
                assert!(!msg.contains("live-traffic"));
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
                assert_eq!(msg, "2 dependent resources block deletion");
                assert!(!msg.contains("live-traffic"));
                assert!(!msg.contains("traffic-stream"));
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

        Mock::given(method("DELETE"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
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
