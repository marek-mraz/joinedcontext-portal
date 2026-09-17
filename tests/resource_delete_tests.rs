use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::api::mutate::branch_name;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::Operation;
use joinedcontext_portal::change::{Change, ChangePhase, Lane};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_CSRF_TOKEN: &str = "test-csrf-token-12345";

fn make_session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@banskabystrica.sk".into()),
            name: Some("Demo Steward".into()),
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
        },
        expires_at: now + 3600,
        issued_at: now,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        let pair = raw.split(';').next().unwrap_or_default();
        parts.push(pair.to_string());
    }
    parts.join("; ")
}

fn session_and_csrf_cookies(config: &Config) -> String {
    let session = make_session_cookie(config);
    format!("{session}; {CSRF_COOKIE}={TEST_CSRF_TOKEN}")
}

#[tokio::test]
async fn delete_returns_202_with_change_and_commits_to_gitea() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

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

    mount_tree_and_commit(&server, &["projects/ovzdusie/spaces/mobility/space.yaml"]).await;

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

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change: Change = serde_json::from_slice(&body_bytes).expect("deserialize Change");
    assert_eq!(change.api_version, API_VERSION);
    assert_eq!(change.kind, "Change");
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

    let requests = server.received_requests().await.expect("received requests");

    let branch_req = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/branches"
        })
        .expect("branch creation request");
    let branch_body: serde_json::Value =
        serde_json::from_slice(&branch_req.body).expect("branch request body");
    assert!(branch_body["new_branch_name"]
        .as_str()
        .expect("new branch name")
        .starts_with("portal/delete-contextspace-mobility-"));
    assert_eq!(branch_body["old_branch_name"], "main");

    let delete_req = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/contents"
        })
        .expect("the commit that removes the files");
    let delete_body: serde_json::Value =
        serde_json::from_slice(&delete_req.body).expect("commit request body");
    assert_eq!(delete_body["author"]["name"], "Demo Steward");
    assert_eq!(
        delete_body["author"]["email"],
        "demo.steward@banskabystrica.sk"
    );
    assert_eq!(delete_body["committer"]["name"], "Demo Steward");
    assert_eq!(
        delete_body["committer"]["email"],
        "demo.steward@banskabystrica.sk"
    );
    let files = delete_body["files"].as_array().expect("the files removed");
    assert_eq!(files.len(), 1, "the manifest alone: {files:?}");
    assert_eq!(files[0]["operation"], "delete");
    assert_eq!(
        files[0]["path"],
        "projects/ovzdusie/spaces/mobility/space.yaml"
    );
    assert_eq!(files[0]["sha"], "sha-space-123");
    assert_eq!(delete_body["message"], "delete ContextSpace mobility");

    let pulls_req = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/pulls"
        })
        .expect("pull request creation request");
    let pulls_body: serde_json::Value =
        serde_json::from_slice(&pulls_req.body).expect("pull request body");
    assert_eq!(pulls_body["title"], "delete ContextSpace mobility");
    assert_eq!(pulls_body["base"], "main");
    assert!(pulls_body["head"]
        .as_str()
        .expect("head branch")
        .starts_with("portal/delete-contextspace-mobility-"));
}

#[tokio::test]
async fn delete_missing_name_returns_404() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/nonexistent")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 404);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/resource-not-found"
    );
    assert_eq!(
        problem.detail.as_deref(),
        Some("resource 'nonexistent' not found in project 'ovzdusie'")
    );
}

#[tokio::test]
async fn delete_unknown_plural_returns_404() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/unknownplurals/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 404);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/resource-not-found"
    );
    assert_eq!(
        problem.detail.as_deref(),
        Some("resource 'mobility' not found in project 'ovzdusie'")
    );
}

/// AG-77, R20: a blocked deletion names the references of the caller's own project, which the
/// caller reads anyway, and would only count those of other projects.
#[tokio::test]
async fn delete_blocked_by_dependents_returns_409_naming_this_projects_references() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Endpoint".to_string(),
        metadata: ObjectMeta {
            name: "live-traffic".to_string(),
            namespace: Some("ovzdusie".to_string()),
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

    let app = server::app(state.clone());

    let response1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response1.status(), StatusCode::CONFLICT);
    assert_eq!(
        response1.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes1 = response1
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem1: ProblemDetails = serde_json::from_slice(&bytes1).expect("ProblemDetails");
    assert_eq!(problem1.status, 409);
    assert_eq!(problem1.r#type, "https://joinedcontext.com/errors/conflict");
    assert_eq!(
        problem1.detail.as_deref(),
        Some("1 dependent resource blocks deletion: Endpoint live-traffic")
    );

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Pipeline".to_string(),
        metadata: ObjectMeta {
            name: "traffic-stream".to_string(),
            namespace: Some("ovzdusie".to_string()),
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

    let response2 = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response2.status(), StatusCode::CONFLICT);
    let bytes2 = response2
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem2: ProblemDetails = serde_json::from_slice(&bytes2).expect("ProblemDetails");
    assert_eq!(problem2.status, 409);
    assert_eq!(
        problem2.detail.as_deref(),
        Some(
            "2 dependent resources block deletion: Endpoint live-traffic, Pipeline traffic-stream"
        )
    );

    let requests = server.received_requests().await.expect("received requests");
    assert!(requests.is_empty());
}

#[tokio::test]
async fn delete_without_forge_returns_503() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 503);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/service-unavailable"
    );
    assert_eq!(
        problem.detail.as_deref(),
        Some("git forge is not configured")
    );
}

#[tokio::test]
async fn delete_without_csrf_header_returns_403() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 403);
    assert_eq!(problem.r#type, "https://joinedcontext.com/errors/forbidden");
}

fn space_state(client: GiteaClient) -> (Config, AppState) {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });
    (config, state)
}

/// The tree the removal reads to find the files the resource owns beside its manifest, and
/// the one commit that removes them (T-0900).
async fn mount_tree_and_commit(server: &MockServer, tree: &[&str]) {
    let entries: Vec<serde_json::Value> = tree
        .iter()
        .map(|path| json!({ "path": path, "type": "blob" }))
        .collect();
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/api/v1/repos/test-owner/test-repo/git/trees/.*$",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "tree": entries, "truncated": false })),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/contents"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "commit": { "sha": "commit-sha-deleted" }
        })))
        .mount(server)
        .await;
}

async fn mount_repo_and_file(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-sha",
            "content": "YXBpVmVyc2lvbjogam9pbmVkY29udGV4dC5jb20vdjFhbHBoYTEK",
            "encoding": "base64"
        })))
        .mount(server)
        .await;
}

async fn delete_mobility(config: &Config, state: AppState) -> axum::response::Response {
    server::app(state)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

/// A branch an earlier attempt left behind is recreated from main, never reused: on dev the
/// stale branch had the file already deleted and the removal answered 404 (T-0886).
#[tokio::test]
async fn a_stale_branch_is_recreated_from_main_before_the_removal_is_written() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    mount_repo_and_file(&server).await;
    let branch = branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Delete);
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    // The first creation finds the stale branch; after it is dropped the second one succeeds.
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(409).set_body_string("branch already exists"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/branches/{branch}"
        )))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    mount_tree_and_commit(&server, &["projects/ovzdusie/spaces/mobility/space.yaml"]).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 56,
            "html_url": "https://gitea.example.sk/pulls/56",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let (config, state) = space_state(client);
    let response = delete_mobility(&config, state).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("received requests");
    let names: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/branches"))
        .map(|r| {
            r.body_json::<serde_json::Value>().expect("branch body")["new_branch_name"]
                .as_str()
                .expect("name")
                .to_string()
        })
        .collect();
    assert_eq!(names[0], branch, "the deterministic name is tried first");
    assert!(
        names[1].starts_with(&format!("{branch}_")) && names[1] != branch,
        "the retry opens on a fresh name: {}",
        names[1]
    );
    let heads: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/pulls"))
        .map(|r| {
            r.body_json::<serde_json::Value>().expect("pull body")["head"]
                .as_str()
                .expect("head")
                .to_string()
        })
        .collect();
    assert_eq!(
        heads,
        vec![names[1].clone()],
        "the pull request opens on the fresh name"
    );
}

/// A removal already under review is decided first: the second one names it (T-0883).
#[tokio::test]
async fn a_second_removal_while_one_is_open_names_the_open_change() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    mount_repo_and_file(&server).await;
    let branch = branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Delete);
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "number": 300,
            "html_url": "https://gitea.example.sk/pulls/300",
            "state": "open",
            "head": { "ref": branch },
            "base": { "ref": "main" },
            "merged": false
        }])))
        .mount(&server)
        .await;

    let (config, state) = space_state(client);
    let response = delete_mobility(&config, state).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&body).expect("problem");
    assert_eq!(
        problem.detail.as_deref(),
        Some("a change for ContextSpace 'mobility' is already open: chg-0000012c; approve or reject it first")
    );
    let requests = server.received_requests().await.expect("received requests");
    assert!(
        requests.iter().all(|r| r.method.as_str() == "GET"),
        "nothing was written"
    );
}

/// T-0900: a DataModel is a manifest plus its LinkML source and whatever was rendered from it.
/// On dev the manifest went and the source stayed, so the next model of that name would have
/// inherited a schema nobody wrote for it.
#[tokio::test]
async fn deleting_a_datamodel_removes_its_source_and_its_artefacts_in_one_change() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    for (file, sha) in [
        ("datamodels/bikes.yaml", "sha-manifest"),
        ("datamodels/bikes.linkml.yaml", "sha-source"),
        ("datamodels/bikes.schema.json", "sha-schema"),
        ("datamodels/shared.linkml.yaml", "sha-shared"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/{file}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": sha,
                "content": "YXBpVmVyc2lvbjogam9pbmVkY29udGV4dC5jb20vdjFhbHBoYTEK",
                "encoding": "base64"
            })))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    mount_tree_and_commit(
        &server,
        &[
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml",
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.linkml.yaml",
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.schema.json",
            // Another model's source, named after it and kept.
            "projects/ovzdusie/spaces/mobility/datamodels/shared.linkml.yaml",
            "projects/ovzdusie/spaces/mobility/space.yaml",
        ],
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 57,
            "html_url": "https://gitea.example.sk/pulls/57",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let (config, state) = space_state(client);
    let model = |name: &str, linkml: &str| ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "contextSpaceRef": "mobility", "linkml": linkml, "version": "0.1.0" }),
        status: None,
    };
    state.mirror.upsert(model("bikes", "./bikes.linkml.yaml"));
    state.mirror.upsert(model("shared", "./shared.linkml.yaml"));

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("received requests");
    let commit = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/contents"
        })
        .expect("the commit that removes the files");
    let body: serde_json::Value =
        serde_json::from_slice(&commit.body).expect("commit request body");
    let removed: Vec<&str> = body["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["path"].as_str().expect("a path"))
        .collect();

    assert!(removed.contains(&"projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml"));
    assert!(removed.contains(&"projects/ovzdusie/spaces/mobility/datamodels/bikes.linkml.yaml"));
    assert!(removed.contains(&"projects/ovzdusie/spaces/mobility/datamodels/bikes.schema.json"));
    assert!(
        !removed
            .iter()
            .any(|path| path.contains("shared") || path.ends_with("space.yaml")),
        "only the model's own files: {removed:?}"
    );
    assert!(body["files"]
        .as_array()
        .expect("files")
        .iter()
        .all(|file| file["operation"] == "delete"));
}

/// The other half of T-0900: a source another model still names is not this model's to remove.
#[tokio::test]
async fn a_source_another_manifest_still_names_survives_the_delete() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    for (file, sha) in [
        ("datamodels/bikes.yaml", "sha-manifest"),
        ("datamodels/bikes.linkml.yaml", "sha-source"),
    ] {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/{file}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": sha,
                "content": "YXBpVmVyc2lvbjogam9pbmVkY29udGV4dC5jb20vdjFhbHBoYTEK",
                "encoding": "base64"
            })))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    mount_tree_and_commit(
        &server,
        &[
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml",
            "projects/ovzdusie/spaces/mobility/datamodels/bikes.linkml.yaml",
        ],
    )
    .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 58,
            "html_url": "https://gitea.example.sk/pulls/58",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let (config, state) = space_state(client);
    let model = |name: &str, linkml: &str| ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "contextSpaceRef": "mobility", "linkml": linkml, "version": "0.1.0" }),
        status: None,
    };
    state.mirror.upsert(model("bikes", "./bikes.linkml.yaml"));
    // A second model that reads the first one's source: the file is not the first one's alone.
    state
        .mirror
        .upsert(model("cargo-bikes", "./bikes.linkml.yaml"));

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("received requests");
    let commit = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/contents"
        })
        .expect("the commit that removes the files");
    let body: serde_json::Value =
        serde_json::from_slice(&commit.body).expect("commit request body");
    let removed: Vec<&str> = body["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["path"].as_str().expect("a path"))
        .collect();
    assert_eq!(
        removed,
        vec!["projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml"],
        "the manifest alone: another model reads that source"
    );
}
