use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::{Change, ChangePhase, Lane};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
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
        },
        expires_at: now + 3600,
        issued_at: now,
        id_token: "id-token-placeholder".into(),
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
            r.method.as_str() == "DELETE"
                && r.url
                    .path()
                    .starts_with("/api/v1/repos/test-owner/test-repo/contents/")
        })
        .expect("DELETE contents request");
    assert_eq!(
        delete_req.url.path(),
        "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml"
    );
    let delete_body: serde_json::Value =
        serde_json::from_slice(&delete_req.body).expect("DELETE request body");
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
    assert_eq!(delete_body["sha"], "sha-space-123");
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

#[tokio::test]
async fn delete_blocked_by_dependents_returns_409_naming_only_count() {
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
        Some("1 dependent resource blocks deletion")
    );
    assert!(!problem1
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("live-traffic"));

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
        Some("2 dependent resources block deletion")
    );
    assert!(!problem2
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("live-traffic"));
    assert!(!problem2
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("traffic-stream"));

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
