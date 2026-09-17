use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use joinedcontext_portal::api::changes::{ChangeList, ChangeProposal};
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::{Change, ChangePhase, Lane};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_CSRF_TOKEN: &str = "test-csrf-token-12345";

fn make_session_cookie(
    config: &Config,
    username: &str,
    email: Option<&str>,
    name: Option<&str>,
    roles: Vec<&str>,
) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            subject: format!("sub-{username}"),
            username: username.to_string(),
            email: email.map(str::to_string),
            name: name.map(str::to_string),
            roles: roles.into_iter().map(str::to_string).collect(),
            groups: Vec::new(),
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

fn session_and_csrf_cookies(
    config: &Config,
    username: &str,
    email: Option<&str>,
    name: Option<&str>,
    roles: Vec<&str>,
) -> String {
    let session = make_session_cookie(config, username, email, name, roles);
    format!("{session}; {CSRF_COOKIE}={TEST_CSRF_TOKEN}")
}

fn approver_cookies(config: &Config) -> String {
    session_and_csrf_cookies(
        config,
        "jana.approver",
        Some("jana.approver@banskabystrica.sk"),
        Some("Jana Approver"),
        vec!["portal-approver"],
    )
}

fn encode_b64(content: &str) -> String {
    STANDARD.encode(content.as_bytes())
}

/// The files a merge request changes, as the forge lists them (T-0832).
async fn pr_files(server: &MockServer, number: u64, files: &[(&str, &str)]) {
    let listed: Vec<Value> = files
        .iter()
        .map(|(filename, status)| json!({ "filename": filename, "status": status }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/pulls/{number}/files"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(listed))
        .mount(server)
        .await;
}

#[tokio::test]
async fn list_changes_filters_portal_prefix_sorts_newest_first_with_metadata() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    // Mock pull requests: PR 1 (portal/), PR 2 (feature/ non-portal), PR 3 (portal/)
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "number": 1,
                "html_url": "https://gitea.example.sk/pulls/1",
                "state": "open",
                "title": "create ContextSpace mobility",
                "head": { "ref": "portal/create-contextspace-mobility-11111111" },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T10:00:00Z",
                "user": {
                    "login": "jana.kovacova",
                    "full_name": "Jana Kováčová",
                    "email": "jana.kovacova@banskabystrica.sk"
                },
                "mergeable": true,
                "merged": false
            },
            {
                "number": 2,
                "html_url": "https://gitea.example.sk/pulls/2",
                "state": "open",
                "title": "manual feature branch",
                "head": { "ref": "feature/custom-branch" },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T11:00:00Z",
                "user": {
                    "login": "other.dev",
                    "full_name": "Other Dev",
                    "email": "other.dev@example.sk"
                },
                "mergeable": true,
                "merged": false
            },
            {
                "number": 3,
                "html_url": "https://gitea.example.sk/pulls/3",
                "state": "open",
                "title": "delete ContextSpace traffic",
                "head": { "ref": "portal/delete-contextspace-traffic-33333333" },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T12:00:00Z",
                "user": {
                    "login": "city.steward",
                    "full_name": "City Steward",
                    "email": "steward@banskabystrica.sk"
                },
                "mergeable": true,
                "merged": false
            }
        ])))
        .mount(&server)
        .await;

    // PR 1 is one manifest; PR 3 is a bundle — the count the list renders beside it.
    pr_files(
        &server,
        1,
        &[("projects/ovzdusie/spaces/mobility/space.yaml", "added")],
    )
    .await;
    pr_files(
        &server,
        3,
        &[
            ("projects/ovzdusie/spaces/traffic/space.yaml", "deleted"),
            ("projects/ovzdusie/pipelines/traffic-in.yaml", "deleted"),
        ],
    )
    .await;

    let space_mobility = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: mobility
  namespace: ovzdusie
spec:
  isSandbox: true
"#;
    // The head commit of PR 1 carries the human author; PR 3's branch answers nothing, so its
    // poster stands in.
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/commits"))
        .and(query_param("sha", "portal/create-contextspace-mobility-11111111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "sha": "0123456789abcdef",
                "commit": {
                    "message": "create ContextSpace mobility",
                    "author": { "name": "Demo Steward", "email": "demo.steward@hel.fi", "date": "2026-09-06T10:00:00Z" }
                }
            }
        ])))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "portal/create-contextspace-mobility-11111111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-sha-mobility",
            "content": encode_b64(space_mobility)
        })))
        .mount(&server)
        .await;

    let space_traffic = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: traffic
  namespace: ovzdusie
spec:
  isSandbox: false
"#;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/traffic/space.yaml",
        ))
        .and(query_param("ref", "main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-sha-traffic",
            "content": encode_b64(space_traffic)
        })))
        .mount(&server)
        .await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/changes")
                .header(header::COOKIE, approver_cookies(&config))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let list: ChangeList = serde_json::from_slice(&bytes).expect("deserialize ChangeList");

    // Only PR 3 and PR 1 match "portal/", PR 2 is excluded
    assert_eq!(list.items.len(), 2);

    // Newest first (created_at 12:00:00Z before 10:00:00Z)
    assert_eq!(list.items[0].metadata.name, "chg-00000003");
    assert_eq!(list.items[0].created_at, "2026-09-06T12:00:00Z");
    assert_eq!(list.items[0].status.lane, Lane::Red);
    assert_eq!(list.items[0].summary.key, "change.summary.delete");
    assert_eq!(list.items[0].summary.params["kind"], "ContextSpace");
    assert_eq!(list.items[0].summary.params["name"], "traffic");
    assert_eq!(list.items[0].author.name, "City Steward");
    assert_eq!(
        list.items[0].author.email.as_deref(),
        Some("steward@banskabystrica.sk")
    );

    assert_eq!(list.items[1].metadata.name, "chg-00000001");
    assert_eq!(list.items[1].created_at, "2026-09-06T10:00:00Z");
    assert_eq!(list.items[1].status.lane, Lane::Green);
    assert_eq!(list.items[1].summary.key, "change.summary.create");
    assert_eq!(list.items[1].summary.params["kind"], "ContextSpace");
    assert_eq!(list.items[1].summary.params["name"], "mobility");
    // PR 1 was posted by the service token; its head commit names the human (CC-44, T-0506).
    assert_eq!(list.items[1].author.name, "Demo Steward");
    assert_eq!(
        list.items[1].author.email.as_deref(),
        Some("demo.steward@hel.fi")
    );
}

#[tokio::test]
async fn get_change_returns_redacted_plan_diff() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    // PR 10 in hex is chg-0000000a
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 10,
            "html_url": "https://gitea.example.sk/pulls/10",
            "state": "open",
            "title": "update ContextSpace mobility",
            "head": { "ref": "portal/update-contextspace-mobility-12345678" },
            "base": { "ref": "main" },
            "created_at": "2026-09-06T09:14:22Z",
            "user": {
                "login": "dev.user",
                "full_name": "Dev User",
                "email": "dev@example.sk"
            },
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let head_yaml = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: mobility
  namespace: ovzdusie
spec:
  isSandbox: true
  auth:
    token: new-token-secret-123
    password: new-password-secret-456
    secret: new-secret-val-789
    clientSecret: new-client-secret-abc
    apiKey: new-api-key-def
"#;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "portal/update-contextspace-mobility-12345678"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-head-sha",
            "content": encode_b64(head_yaml)
        })))
        .mount(&server)
        .await;

    let base_yaml = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: mobility
  namespace: ovzdusie
spec:
  isSandbox: false
  auth:
    token: old-token-secret-123
    password: old-password-secret-456
    secret: old-secret-val-789
    clientSecret: old-client-secret-abc
    apiKey: old-api-key-def
"#;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-base-sha",
            "content": encode_b64(base_yaml)
        })))
        .mount(&server)
        .await;

    pr_files(
        &server,
        10,
        &[("projects/ovzdusie/spaces/mobility/space.yaml", "modified")],
    )
    .await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/changes/chg-0000000a")
                .header(header::COOKIE, approver_cookies(&config))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let proposal: ChangeProposal =
        serde_json::from_slice(&bytes).expect("deserialize ChangeProposal");

    assert_eq!(proposal.metadata.name, "chg-0000000a");
    let fields = proposal.plan_fields.expect("plan_fields should be set");

    let field_map: std::collections::HashMap<
        String,
        (Option<serde_json::Value>, Option<serde_json::Value>),
    > = fields
        .iter()
        .map(|f| (f.path.clone(), (f.from.clone(), f.to.clone())))
        .collect();

    // Verify all 5 sensitive fields have [REDACTED] in both from and to while paths are visible
    assert_eq!(
        field_map.get("spec.auth.token"),
        Some(&(Some(json!("[REDACTED]")), Some(json!("[REDACTED]"))))
    );
    assert_eq!(
        field_map.get("spec.auth.password"),
        Some(&(Some(json!("[REDACTED]")), Some(json!("[REDACTED]"))))
    );
    assert_eq!(
        field_map.get("spec.auth.secret"),
        Some(&(Some(json!("[REDACTED]")), Some(json!("[REDACTED]"))))
    );
    assert_eq!(
        field_map.get("spec.auth.clientSecret"),
        Some(&(Some(json!("[REDACTED]")), Some(json!("[REDACTED]"))))
    );
    assert_eq!(
        field_map.get("spec.auth.apiKey"),
        Some(&(Some(json!("[REDACTED]")), Some(json!("[REDACTED]"))))
    );

    // Non-sensitive field is NOT redacted
    assert_eq!(
        field_map.get("spec.isSandbox"),
        Some(&(Some(json!(false)), Some(json!(true))))
    );

    // Confirm that the raw secrets do not appear anywhere in the serialized payload
    let raw_text = String::from_utf8(bytes.to_vec()).expect("utf8 string");
    assert!(!raw_text.contains("old-token-secret-123"));
    assert!(!raw_text.contains("new-token-secret-123"));
    assert!(!raw_text.contains("old-password-secret-456"));
    assert!(!raw_text.contains("new-password-secret-456"));
    assert!(!raw_text.contains("old-secret-val-789"));
    assert!(!raw_text.contains("new-secret-val-789"));
    assert!(!raw_text.contains("old-client-secret-abc"));
    assert!(!raw_text.contains("new-client-secret-abc"));
    assert!(!raw_text.contains("old-api-key-def"));
    assert!(!raw_text.contains("new-api-key-def"));
}

#[tokio::test]
async fn invalid_change_id_returns_400() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let invalid_ids = [
        "not-a-change",
        "chg-1",
        "chg-000000001",
        "chg-0000000Z",
        "chg-0000001A",
        "mr-00000001",
    ];

    for invalid_id in invalid_ids {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/projects/ovzdusie/changes/{invalid_id}"))
                    .header(header::COOKIE, approver_cookies(&config))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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
        let problem: ProblemDetails =
            serde_json::from_slice(&bytes).expect("deserialize ProblemDetails");
        assert_eq!(problem.status, 400);
        assert_eq!(
            problem.r#type,
            "https://joinedcontext.com/errors/invalid-request"
        );
        assert!(problem
            .detail
            .as_deref()
            .unwrap()
            .contains("invalid change id"));
        assert!(problem
            .detail
            .as_deref()
            .unwrap()
            .contains("expected 'chg-' followed by 8 lowercase hex digits"));
    }
}

#[tokio::test]
async fn approve_without_portal_approver_role_returns_403() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let viewer_cookies = session_and_csrf_cookies(
        &config,
        "jana.viewer",
        Some("jana.viewer@banskabystrica.sk"),
        Some("Jana Viewer"),
        vec!["portal-viewer"],
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000001/approve")
                .header(header::COOKIE, viewer_cookies)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
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
    let problem: ProblemDetails =
        serde_json::from_slice(&bytes).expect("deserialize ProblemDetails");
    assert_eq!(problem.status, 403);
    assert_eq!(problem.r#type, "https://joinedcontext.com/errors/forbidden");

    let requests = server.received_requests().await.expect("received requests");
    assert!(requests.is_empty());
}

#[tokio::test]
async fn approve_self_approval_returns_403_and_makes_no_git_mutations() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    // PR author is jana.kovacova@banskabystrica.sk
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 1,
            "html_url": "https://gitea.example.sk/pulls/1",
            "state": "open",
            "title": "create ContextSpace mobility",
            "head": { "ref": "portal/create-contextspace-mobility-11111111" },
            "base": { "ref": "main" },
            "created_at": "2026-09-06T09:14:22Z",
            "user": {
                "login": "jana.kovacova",
                "full_name": "Jana Kováčová",
                "email": "jana.kovacova@banskabystrica.sk"
            },
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let space_yaml = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: mobility
  namespace: ovzdusie
spec:
  isSandbox: true
"#;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "portal/create-contextspace-mobility-11111111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": encode_b64(space_yaml)
        })))
        .mount(&server)
        .await;
    pr_files(
        &server,
        1,
        &[("projects/ovzdusie/spaces/mobility/space.yaml", "added")],
    )
    .await;

    // Caller is the author and also has portal-approver role
    let author_approver_cookies = session_and_csrf_cookies(
        &config,
        "jana.kovacova",
        Some("jana.kovacova@banskabystrica.sk"),
        Some("Jana Kováčová"),
        vec!["portal-approver"],
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000001/approve")
                .header(header::COOKIE, author_approver_cookies)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
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
    let problem: ProblemDetails =
        serde_json::from_slice(&bytes).expect("deserialize ProblemDetails");
    assert_eq!(problem.status, 403);
    assert!(problem.r#type.ends_with("self-approval"));
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/self-approval"
    );
    assert!(problem
        .detail
        .as_deref()
        .unwrap()
        .contains("proposal author cannot approve their own change (AG-11)"));

    // Crucial: mock saw NO reviews and NO merges
    let requests = server.received_requests().await.expect("received requests");
    assert!(!requests.iter().any(|r| r.url.path().ends_with("/reviews")));
    assert!(!requests.iter().any(|r| r.url.path().ends_with("/merge")));
}

#[tokio::test]
async fn reject_own_proposal_is_allowed() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 1,
            "html_url": "https://gitea.example.sk/pulls/1",
            "state": "open",
            "title": "create ContextSpace mobility",
            "head": { "ref": "portal/create-contextspace-mobility-11111111" },
            "base": { "ref": "main" },
            "created_at": "2026-09-06T09:14:22Z",
            "user": {
                "login": "jana.kovacova",
                "full_name": "Jana Kováčová",
                "email": "jana.kovacova@banskabystrica.sk"
            },
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let space_yaml = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: mobility
  namespace: ovzdusie
spec:
  isSandbox: true
"#;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "portal/create-contextspace-mobility-11111111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": encode_b64(space_yaml)
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/1/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    // Caller is the author and rejects their own change proposal
    let author_approver_cookies = session_and_csrf_cookies(
        &config,
        "jana.kovacova",
        Some("jana.kovacova@banskabystrica.sk"),
        Some("Jana Kováčová"),
        vec!["portal-approver"],
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000001/reject")
                .header(header::COOKIE, author_approver_cookies)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change: Change = serde_json::from_slice(&bytes).expect("deserialize Change");

    assert_eq!(change.metadata.name, "chg-00000001");
    assert_eq!(change.status.phase, ChangePhase::Rejected);

    // Mock saw one COMMENT review naming the rejecter (the forge token authored the pull, so
    // a REQUEST_CHANGES verdict would be refused as a self-review), the pull closed, no merge.
    let requests = server.received_requests().await.expect("received requests");
    let reviews: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/reviews"))
        .collect();
    assert_eq!(reviews.len(), 1);
    let review_body: serde_json::Value =
        serde_json::from_slice(&reviews[0].body).expect("review json body");
    assert_eq!(review_body["event"], "COMMENT");
    assert!(review_body["body"]
        .as_str()
        .unwrap_or_default()
        .contains("jana.kovacova@banskabystrica.sk"));
    assert!(requests
        .iter()
        .any(|r| r.method == "PATCH" && r.url.path().ends_with("/pulls/1")));
    assert!(!requests.iter().any(|r| r.url.path().ends_with("/merge")));
}

#[tokio::test]
async fn approve_red_lane_requires_confirm_and_records_git_actions() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    // Red lane change: deletion of ContextSpace mobility
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 2,
            "html_url": "https://gitea.example.sk/pulls/2",
            "state": "open",
            "title": "delete ContextSpace mobility",
            "head": { "ref": "portal/delete-contextspace-mobility-22222222" },
            "base": { "ref": "main" },
            "created_at": "2026-09-06T09:14:22Z",
            "user": {
                "login": "different.user",
                "full_name": "Different User",
                "email": "different.user@banskabystrica.sk"
            },
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let space_yaml = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: mobility
  namespace: ovzdusie
spec:
  isSandbox: true
"#;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": encode_b64(space_yaml)
        })))
        .mount(&server)
        .await;
    pr_files(
        &server,
        2,
        &[("projects/ovzdusie/spaces/mobility/space.yaml", "deleted")],
    )
    .await;

    // 1. Missing confirm body -> 400 Bad Request
    let resp_no_confirm = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000002/approve")
                .header(header::COOKIE, approver_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_no_confirm.status(), StatusCode::BAD_REQUEST);
    let bytes_no_confirm = resp_no_confirm
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let prob_no_confirm: ProblemDetails = serde_json::from_slice(&bytes_no_confirm).unwrap();
    assert_eq!(prob_no_confirm.status, 400);
    assert_eq!(
        prob_no_confirm.r#type,
        "https://joinedcontext.com/errors/invalid-request"
    );
    assert!(prob_no_confirm
        .detail
        .as_deref()
        .unwrap()
        .contains("red lane change requires confirm to be 'mobility' (CC-19, CC-39)"));

    let reqs_after_1 = server.received_requests().await.expect("received requests");
    assert!(!reqs_after_1
        .iter()
        .any(|r| r.url.path().ends_with("/reviews")));
    assert!(!reqs_after_1
        .iter()
        .any(|r| r.url.path().ends_with("/merge")));

    // 2. Wrong confirm body -> 400 Bad Request
    let wrong_body = json!({ "confirm": "wrong-resource-name" });
    let resp_wrong_confirm = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000002/approve")
                .header(header::COOKIE, approver_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&wrong_body).unwrap()))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_wrong_confirm.status(), StatusCode::BAD_REQUEST);
    let bytes_wrong_confirm = resp_wrong_confirm
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let prob_wrong_confirm: ProblemDetails = serde_json::from_slice(&bytes_wrong_confirm).unwrap();
    assert!(prob_wrong_confirm
        .detail
        .as_deref()
        .unwrap()
        .contains("red lane change requires confirm to be 'mobility' (CC-19, CC-39)"));

    let reqs_after_2 = server.received_requests().await.expect("received requests");
    assert!(!reqs_after_2
        .iter()
        .any(|r| r.url.path().ends_with("/reviews")));
    assert!(!reqs_after_2
        .iter()
        .any(|r| r.url.path().ends_with("/merge")));

    // 3. Right confirm body -> 202 Accepted and triggers review + merge
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

    let right_body = json!({ "confirm": "mobility" });
    let resp_right_confirm = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000002/approve")
                .header(header::COOKIE, approver_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&right_body).unwrap()))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_right_confirm.status(), StatusCode::ACCEPTED);
    let bytes_right = resp_right_confirm
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change: Change = serde_json::from_slice(&bytes_right).unwrap();
    assert_eq!(change.metadata.name, "chg-00000002");
    assert_eq!(change.status.phase, ChangePhase::Deploying);
    assert_eq!(change.status.lane, Lane::Red);

    // Mock saw review and merge only in this last successful case
    let reqs_final = server.received_requests().await.expect("received requests");
    let reviews: Vec<_> = reqs_final
        .iter()
        .filter(|r| r.url.path().ends_with("/reviews"))
        .collect();
    let merges: Vec<_> = reqs_final
        .iter()
        .filter(|r| r.url.path().ends_with("/merge"))
        .collect();

    // No forge review: the forge token authored the pull, so the approver is recorded in the
    // merge message instead (T-0648).
    assert_eq!(reviews.len(), 0);
    assert_eq!(merges.len(), 1);

    let merge_body: serde_json::Value = serde_json::from_slice(&merges[0].body).unwrap();
    assert!(merge_body["merge_message_field"]
        .as_str()
        .unwrap_or_default()
        .contains("Approved in the Portal by"));
    assert_eq!(merge_body["Do"], "squash");
}

#[tokio::test]
async fn successful_approve_answers_202_deploying() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    // Green lane change proposal: create sandbox ContextSpace
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 3,
            "html_url": "https://gitea.example.sk/pulls/3",
            "state": "open",
            "title": "create ContextSpace sandbox-space",
            "head": { "ref": "portal/create-contextspace-sandbox-space-33333333" },
            "base": { "ref": "main" },
            "created_at": "2026-09-06T09:14:22Z",
            "user": {
                "login": "different.author",
                "full_name": "Different Author",
                "email": "author@banskabystrica.sk"
            },
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let space_yaml = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: sandbox-space
  namespace: ovzdusie
spec:
  isSandbox: true
"#;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/sandbox-space/space.yaml",
        ))
        .and(query_param("ref", "portal/create-contextspace-sandbox-space-33333333"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": encode_b64(space_yaml)
        })))
        .mount(&server)
        .await;
    pr_files(
        &server,
        3,
        &[("projects/ovzdusie/spaces/sandbox-space/space.yaml", "added")],
    )
    .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/3/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/3/merge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000003/approve")
                .header(header::COOKIE, approver_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change: Change = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(change.metadata.name, "chg-00000003");
    assert_eq!(change.status.phase, ChangePhase::Deploying);
    assert_eq!(change.status.lane, Lane::Green);
}

#[tokio::test]
async fn reject_answers_202_rejected_and_requests_changes_without_merge() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 4,
            "html_url": "https://gitea.example.sk/pulls/4",
            "state": "open",
            "title": "create ContextSpace mobility",
            "head": { "ref": "portal/create-contextspace-mobility-44444444" },
            "base": { "ref": "main" },
            "created_at": "2026-09-06T09:14:22Z",
            "user": {
                "login": "author.dev",
                "full_name": "Author Dev",
                "email": "author@example.sk"
            },
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let space_yaml = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: mobility
  namespace: ovzdusie
spec:
  isSandbox: true
"#;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "portal/create-contextspace-mobility-44444444"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": encode_b64(space_yaml)
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/4/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000004/reject")
                .header(header::COOKIE, approver_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change: Change = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(change.metadata.name, "chg-00000004");
    assert_eq!(change.status.phase, ChangePhase::Rejected);

    // Mock saw one COMMENT review naming the rejecter, the pull closed, and zero merges
    // (T-0648: the forge token authored the pull, so a REQUEST_CHANGES verdict is refused).
    let requests = server.received_requests().await.expect("received requests");
    let reviews: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/reviews"))
        .collect();
    assert_eq!(reviews.len(), 1);

    let review_body: serde_json::Value = serde_json::from_slice(&reviews[0].body).unwrap();
    assert_eq!(review_body["event"], "COMMENT");
    assert!(review_body["body"]
        .as_str()
        .unwrap_or_default()
        .starts_with("Change proposal rejected in the Portal by "));
    assert!(requests
        .iter()
        .any(|r| r.method == "PATCH" && r.url.path().ends_with("/pulls/4")));
    assert!(!requests.iter().any(|r| r.url.path().ends_with("/merge")));
}

#[tokio::test]
async fn list_changes_without_forge_answers_503() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/changes")
                .header(header::COOKIE, approver_cookies(&config))
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
    let problem: ProblemDetails = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(problem.status, 503);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/service-unavailable"
    );
    assert!(problem
        .detail
        .as_deref()
        .unwrap()
        .contains("git forge is not configured"));
}

#[tokio::test]
async fn approve_without_csrf_header_returns_403() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    // Approver session cookie is present, but X-CSRF-Token header is missing
    let approver_session_only = make_session_cookie(
        &config,
        "jana.approver",
        Some("jana.approver@banskabystrica.sk"),
        Some("Jana Approver"),
        vec!["portal-approver"],
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000001/approve")
                .header(header::COOKIE, approver_session_only)
                .header(header::CONTENT_TYPE, "application/json")
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
    let problem: ProblemDetails = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(problem.status, 403);
    assert_eq!(problem.r#type, "https://joinedcontext.com/errors/forbidden");
}

/// PF-58: the pull request of `author` creating ContextSpace `mobility`, and a state whose
/// mirror binds `author` to a role of `verbs` on ContextSpace across the organization.
async fn own_change_of(verbs: &[&str]) -> (MockServer, AppState) {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    let server = MockServer::start().await;
    let client = GiteaClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
    let org = |kind: &str, name: &str, spec: serde_json::Value| ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    };
    state.mirror.upsert(org(
        "Role",
        "space-role",
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": verbs }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "space-binding",
        json!({
            "subjects": [{ "user": "jana.kovacova@banskabystrica.sk" }],
            "role": "space-role",
            "scope": { "organization": "bb" }
        }),
    ));

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 1,
            "html_url": "https://gitea.example.sk/pulls/1",
            "state": "open",
            "title": "create ContextSpace mobility",
            "head": { "ref": "portal/create-contextspace-mobility-11111111" },
            "base": { "ref": "main" },
            "created_at": "2026-09-06T09:14:22Z",
            "user": {
                "login": "jana.kovacova",
                "full_name": "Jana Kováčová",
                "email": "jana.kovacova@banskabystrica.sk"
            },
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "portal/create-contextspace-mobility-11111111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": encode_b64("apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n")
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/1/merge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    pr_files(
        &server,
        1,
        &[("projects/ovzdusie/spaces/mobility/space.yaml", "added")],
    )
    .await;
    (server, state)
}

async fn approve_own(state: AppState) -> axum::response::Response {
    let config = state.config.clone();
    let cookies = session_and_csrf_cookies(
        &config,
        "jana.kovacova",
        Some("jana.kovacova@banskabystrica.sk"),
        Some("Jana Kováčová"),
        vec![],
    );
    server::app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000001/approve")
                .header(header::COOKIE, cookies)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({ "confirm": "mobility" }).to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
}

#[tokio::test]
async fn an_administrator_of_the_kind_approves_their_own_change_and_the_merge_says_so() {
    let (server, state) = own_change_of(&["propose", "approve", "delete"]).await;

    let response = approve_own(state).await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("requests");
    let merge = requests
        .iter()
        .find(|r| r.url.path().ends_with("/pulls/1/merge"))
        .expect("the change was merged");
    let body = String::from_utf8_lossy(&merge.body);
    assert!(
        body.contains("its author, as an administrator of ContextSpace (PF-58)"),
        "{body}"
    );
}

#[tokio::test]
async fn an_author_who_approves_but_may_not_delete_still_cannot_approve_their_own_change() {
    let (server, state) = own_change_of(&["propose", "approve"]).await;

    let response = approve_own(state).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let requests = server.received_requests().await.expect("requests");
    assert!(!requests.iter().any(|r| r.url.path().ends_with("/merge")));
}

#[tokio::test]
async fn an_operation_never_approves_its_callers_own_change_even_for_an_administrator() {
    let (server, state) = own_change_of(&["propose", "approve", "delete"]).await;
    let identity = Identity {
        subject: "sub-jana.kovacova".into(),
        username: "jana.kovacova".into(),
        email: Some("jana.kovacova@banskabystrica.sk".into()),
        name: Some("Jana Kováčová".into()),
        roles: Vec::new(),
        groups: Vec::new(),
    };

    let refused = joinedcontext_portal::api::changes::approve_change_for(
        &state,
        &identity,
        "ovzdusie",
        "chg-00000001",
        Some("mobility"),
        joinedcontext_portal::api::changes::ApprovedBy::Operation,
    )
    .await;

    assert!(matches!(
        refused,
        Err(joinedcontext_portal::error::ApiError::SelfApproval(_))
    ));
    let requests = server.received_requests().await.expect("requests");
    assert!(!requests.iter().any(|r| r.url.path().ends_with("/merge")));
}

// --- a merge request is approved whole, not by its headline (T-0832, MF-21, CC-63) ----------

const BUNDLE_BRANCH: &str = "portal/create-pipeline-aq-77777777";
const PIPELINE_PATH: &str = "projects/ovzdusie/pipelines/aq/pipeline.yaml";
const PIPELINE_YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: aq\n  namespace: ovzdusie\nspec:\n  class: resident\n  targetEndpoint: urn:ngsi-ld:Endpoint:bb.sk:ovzdusie:air\n";

/// A merge request by somebody else, headed by the Pipeline `aq` and carrying `extra` files
/// beside it, and an approver whose only rights are `rules` over the organization.
async fn bundle_of(rules: Value, extra: &[(&str, &str)]) -> (MockServer, AppState) {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    let server = MockServer::start().await;
    let client = GiteaClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
    let org = |kind: &str, name: &str, spec: Value| ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    };
    state
        .mirror
        .upsert(org("Role", "reviewer", json!({ "rules": rules })));
    // The role the smuggled binding grants: more than the reviewer holds.
    state.mirror.upsert(org(
        "Role",
        "org-admin",
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "reviewer-binding",
        json!({
            "subjects": [{ "user": "jana.approver@banskabystrica.sk" }],
            "role": "reviewer",
            "scope": { "organization": "bb" }
        }),
    ));

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 7,
            "html_url": "https://gitea.example.sk/pulls/7",
            "state": "open",
            "title": "import bundle",
            "head": { "ref": BUNDLE_BRANCH },
            "base": { "ref": "main" },
            "created_at": "2026-09-15T09:14:22Z",
            "user": { "login": "someone", "full_name": "Someone Else", "email": "someone@banskabystrica.sk" },
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;
    let mut files = vec![(PIPELINE_PATH, "added")];
    files.extend(extra.iter().map(|(file, _)| (*file, "added")));
    pr_files(&server, 7, &files).await;
    for (file, content) in std::iter::once(&(PIPELINE_PATH, PIPELINE_YAML)).chain(extra) {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/test-owner/test-repo/contents/{file}"
            )))
            .and(query_param("ref", BUNDLE_BRANCH))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": format!("blob-{}", file.len()),
                "content": encode_b64(content)
            })))
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/7/merge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    (server, state)
}

/// The reviewer approves change 7, with `confirm` when given.
async fn approve_bundle(state: AppState, confirm: Option<&str>) -> (StatusCode, String) {
    let config = state.config.clone();
    let cookies = session_and_csrf_cookies(
        &config,
        "jana.approver",
        Some("jana.approver@banskabystrica.sk"),
        Some("Jana Approver"),
        vec![],
    );
    let body = match confirm {
        Some(name) => Body::from(json!({ "confirm": name }).to_string()),
        None => Body::empty(),
    };
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000007/approve")
                .header(header::COOKIE, cookies)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn merged(server: &MockServer) -> bool {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .any(|r| r.url.path().ends_with("/pulls/7/merge"))
}

const SMUGGLED_BINDING: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: RoleBinding\nmetadata:\n  name: mallory-admin\n  namespace: org\nspec:\n  subjects:\n    - user: mallory@banskabystrica.sk\n  role: org-admin\n  scope:\n    organization: bb\n";

#[tokio::test]
async fn a_bundle_headed_by_a_pipeline_cannot_smuggle_a_rolebinding_past_a_pipeline_approver() {
    let (server, state) = bundle_of(
        json!([{ "kinds": ["Pipeline"], "verbs": ["approve"] }]),
        &[("users/assignments/mallory-admin.yaml", SMUGGLED_BINDING)],
    )
    .await;
    let (status, body) = approve_bundle(state, Some("aq")).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("RoleBinding"), "{body}");
    assert!(!merged(&server).await);
}

#[tokio::test]
async fn an_approver_of_bindings_still_may_not_approve_one_that_grants_more_than_they_hold() {
    let (server, state) = bundle_of(
        json!([{ "kinds": ["Pipeline", "RoleBinding"], "verbs": ["approve"] }]),
        &[("users/assignments/mallory-admin.yaml", SMUGGLED_BINDING)],
    )
    .await;
    let (status, body) = approve_bundle(state, Some("aq")).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("PF-52"), "{body}");
    assert!(!merged(&server).await);
}

#[tokio::test]
async fn a_red_lane_manifest_inside_a_yellow_bundle_needs_the_confirmation() {
    let policy = "apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nmetadata:\n  name: open\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: mobility\n";
    let rules = json!([{ "kinds": ["Pipeline", "Policy"], "verbs": ["approve"] }]);
    let extra = [(
        "projects/ovzdusie/spaces/mobility/policies/open.yaml",
        policy,
    )];

    let (server, state) = bundle_of(rules.clone(), &extra).await;
    let (status, body) = approve_bundle(state, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("red lane change requires confirm to be 'aq'"),
        "{body}"
    );
    assert!(!merged(&server).await);

    let (server, state) = bundle_of(rules, &extra).await;
    let (status, body) = approve_bundle(state, Some("aq")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(merged(&server).await);
}

#[tokio::test]
async fn a_native_file_under_a_directory_that_names_no_kind_blocks_the_whole_merge_request() {
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    let (server, state) = bundle_of(
        json!([{ "kinds": kinds, "verbs": ["approve"] }]),
        &[("projects/ovzdusie/notes/todo.txt", "remember the milk\n")],
    )
    .await;
    let (status, body) = approve_bundle(state, Some("aq")).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("notes/todo.txt"), "{body}");
    assert!(!merged(&server).await);
}

#[tokio::test]
async fn a_native_file_of_a_granted_kind_travels_with_the_bundle() {
    let (server, state) = bundle_of(
        json!([{ "kinds": ["Pipeline"], "verbs": ["approve"] }]),
        &[(
            "projects/ovzdusie/pipelines/aq/bento.yaml",
            "input:\n  mqtt:\n    urls: [ mqtts://x:8883 ]\n",
        )],
    )
    .await;
    let (status, body) = approve_bundle(state, Some("aq")).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(merged(&server).await);
}

// ---------------------------------------------------------------------------
// A change carries the manifests, so reading one is reading the resource (T-0918, PF-59)
// ---------------------------------------------------------------------------

/// The `own_change_of` fixture plus the listing call, so both change reads can be asked of the
/// same open merge request.
async fn readable_changes_fixture(kinds: &[&str]) -> (MockServer, AppState) {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    let (server, state) = own_change_of(&["read"]).await;
    // A second person, bound to the kinds named here and to nothing else.
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Role".to_owned(),
        metadata: ObjectMeta::new("narrow-role", ORG_NAMESPACE),
        spec: json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
        status: None,
    });
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "RoleBinding".to_owned(),
        metadata: ObjectMeta::new("narrow-binding", ORG_NAMESPACE),
        spec: json!({
            "subjects": [{ "user": "peter.narrow@banskabystrica.sk" }],
            "role": "narrow-role",
            "scope": { "organization": "bb" }
        }),
        status: None,
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "number": 1,
                "html_url": "https://gitea.example.sk/pulls/1",
                "state": "open",
                "title": "create ContextSpace mobility",
                "head": { "ref": "portal/create-contextspace-mobility-11111111" },
                "base": { "ref": "main" },
                "created_at": "2026-09-06T09:14:22Z",
                "user": {
                    "login": "jana.kovacova",
                    "full_name": "Jana Kováčová",
                    "email": "jana.kovacova@banskabystrica.sk"
                },
                "mergeable": true,
                "merged": false
            }
        ])))
        .mount(&server)
        .await;
    (server, state)
}

async fn read_as(state: &AppState, username: &str, email: &str, path: &str) -> (StatusCode, Value) {
    let cookies = session_and_csrf_cookies(&state.config, username, Some(email), None, vec![]);
    let app = server::app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookies)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_person_no_binding_covers_reads_no_change_at_all() {
    let (_server, state) = readable_changes_fixture(&["ContextSpace"]).await;

    let (status, body) = read_as(
        &state,
        "nobody",
        "nobody@banskabystrica.sk",
        "/api/v1/projects/ovzdusie/changes",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let (status, body) = read_as(
        &state,
        "nobody",
        "nobody@banskabystrica.sk",
        "/api/v1/projects/ovzdusie/changes/chg-00000001",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn a_change_to_a_kind_the_caller_does_not_read_is_not_there() {
    let (_server, state) = readable_changes_fixture(&["Pipeline"]).await;

    // The person bound to ContextSpace sees the change the fixture opened.
    let (status, body) = read_as(
        &state,
        "jana.kovacova",
        "jana.kovacova@banskabystrica.sk",
        "/api/v1/projects/ovzdusie/changes",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list: ChangeList = serde_json::from_value(body).expect("a change list");
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].metadata.name, "chg-00000001");

    let (status, body) = read_as(
        &state,
        "jana.kovacova",
        "jana.kovacova@banskabystrica.sk",
        "/api/v1/projects/ovzdusie/changes/chg-00000001",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let one: ChangeProposal = serde_json::from_value(body).expect("a change");
    assert_eq!(one.summary.params.get("kind"), Some(&json!("ContextSpace")));

    // The person bound to Pipeline alone reads the project, and no ContextSpace in it: the
    // change is out of the list, and asking for it by name is a 404, not a 403 (R20).
    let (status, body) = read_as(
        &state,
        "peter.narrow",
        "peter.narrow@banskabystrica.sk",
        "/api/v1/projects/ovzdusie/changes",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list: ChangeList = serde_json::from_value(body).expect("a change list");
    assert!(list.items.is_empty(), "{list:?}");

    let (status, body) = read_as(
        &state,
        "peter.narrow",
        "peter.narrow@banskabystrica.sk",
        "/api/v1/projects/ovzdusie/changes/chg-00000001",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let problem: ProblemDetails = serde_json::from_value(body).expect("a problem");
    let detail = problem.detail.unwrap_or_default();
    assert!(
        !detail.to_lowercase().contains("contextspace"),
        "the refusal names the kind it hid: {detail}"
    );
}

// ---------------------------------------------------------------------------
// Letting data out to the public needs the publisher (EP-76, PF-71, PF-72, T-0874)
// ---------------------------------------------------------------------------

const PUBLIC_ENDPOINT_BRANCH: &str = "portal/create-endpoint-air-00000042";
const PUBLIC_ENDPOINT_PATH: &str = "projects/ovzdusie/spaces/ovzdusie/endpoints/air.yaml";

fn endpoint_yaml(audience: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air\n  \
         namespace: ovzdusie\nspec:\n  contextSpaceRef: ovzdusie\n  slug: \
         mluyob4nz52lok3ssk7pgn5vwt\n  audience: {audience}\n  enabledRepresentations:\n    - \
         ngsi-ld\n"
    )
}

/// One open change that gives the `air` endpoint `audience`, with `approver` bound to `rules`
/// over the organization.
async fn endpoint_change_of(audience: &str, rules: Value) -> (MockServer, AppState) {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    let server = MockServer::start().await;
    let client = GiteaClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
    let org = |kind: &str, name: &str, spec: Value| ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    };
    state
        .mirror
        .upsert(org("Role", "the-role", json!({ "rules": rules })));
    state.mirror.upsert(org(
        "RoleBinding",
        "approver-binding",
        json!({
            "subjects": [{ "user": "jana.approver@banskabystrica.sk" }],
            "role": "the-role",
            "scope": { "organization": "bb" }
        }),
    ));

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/66"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 66,
            "html_url": "https://gitea.example.sk/pulls/66",
            "state": "open",
            "title": "share air quality",
            "head": { "ref": PUBLIC_ENDPOINT_BRANCH },
            "base": { "ref": "main" },
            "created_at": "2026-09-16T09:14:22Z",
            "user": { "login": "someone", "full_name": "Someone Else", "email": "someone@banskabystrica.sk" },
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;
    pr_files(&server, 66, &[(PUBLIC_ENDPOINT_PATH, "added")]).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/contents/{PUBLIC_ENDPOINT_PATH}"
        )))
        .and(query_param("ref", PUBLIC_ENDPOINT_BRANCH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-66", "content": encode_b64(&endpoint_yaml(audience))
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls/66/merge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    (server, state)
}

async fn approve_endpoint(state: AppState, confirm: Option<&str>) -> (StatusCode, String) {
    let config = state.config.clone();
    let cookies = session_and_csrf_cookies(
        &config,
        "jana.approver",
        Some("jana.approver@banskabystrica.sk"),
        Some("Jana Approver"),
        vec![],
    );
    let body = match confirm {
        Some(name) => Body::from(json!({ "confirm": name }).to_string()),
        None => Body::empty(),
    };
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000042/approve")
                .header(header::COOKIE, cookies)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// The seeded steward: everything in the project, and Endpoint only where the audience is not
/// public (PF-71).
fn steward_rules() -> Value {
    json!([
        { "kinds": ["Pipeline", "DataSource"], "verbs": ["propose", "approve"] },
        { "kinds": ["Endpoint"], "verbs": ["propose", "approve"],
          "constraints": [{ "field": "spec.audience", "notIn": ["public"] }] }
    ])
}

/// The seeded publisher: reads the project, approves an Endpoint only where it is public.
fn publisher_rules() -> Value {
    json!([
        { "kinds": ["Endpoint", "Pipeline"], "verbs": ["read"] },
        { "kinds": ["Endpoint"], "verbs": ["approve"],
          "constraints": [{ "field": "spec.audience", "in": ["public"] }] }
    ])
}

#[tokio::test]
async fn a_steward_cannot_approve_a_public_endpoint_and_the_refusal_names_publisher() {
    let (server, state) = endpoint_change_of("public", steward_rules()).await;
    let (status, body) = approve_endpoint(state, Some("air")).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("publisher"), "{body}");
    assert!(body.contains("EP-76"), "{body}");
    assert!(!merged_66(&server).await);
}

#[tokio::test]
async fn a_steward_approves_the_same_endpoint_while_it_is_not_public() {
    let (server, state) = endpoint_change_of("organization", steward_rules()).await;
    let (status, body) = approve_endpoint(state, Some("air")).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(merged_66(&server).await);
}

#[tokio::test]
async fn a_publisher_approves_the_public_one_and_not_the_private_one() {
    let (server, state) = endpoint_change_of("public", publisher_rules()).await;
    let (status, body) = approve_endpoint(state, Some("air")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(merged_66(&server).await);

    let (server, state) = endpoint_change_of("organization", publisher_rules()).await;
    let (status, body) = approve_endpoint(state, Some("air")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(!merged_66(&server).await);
}

#[tokio::test]
async fn an_org_admin_approves_both_and_the_public_one_still_needs_the_name_typed_back() {
    let admin = json!([{ "kinds": ["Endpoint"], "verbs": ["propose", "approve", "delete"] }]);

    // Red lane: without the typed confirmation nothing is merged (CC-19).
    let (server, state) = endpoint_change_of("public", admin.clone()).await;
    let (status, body) = approve_endpoint(state, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!merged_66(&server).await);

    let (server, state) = endpoint_change_of("public", admin.clone()).await;
    let (status, body) = approve_endpoint(state, Some("air")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(merged_66(&server).await);

    let (server, state) = endpoint_change_of("organization", admin).await;
    let (status, body) = approve_endpoint(state, Some("air")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(merged_66(&server).await);
}

async fn merged_66(server: &MockServer) -> bool {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .any(|r| r.url.path().ends_with("/pulls/66/merge"))
}

/// EP-76: the operations registry and the assistant refuse it in the same words as the page,
/// because every door asks the same permission check.
#[tokio::test]
async fn the_operation_refuses_a_public_endpoint_in_the_same_words() {
    let (server, state) = endpoint_change_of("public", steward_rules()).await;
    let identity = Identity {
        subject: "sub-jana.approver".into(),
        username: "jana.approver".into(),
        email: Some("jana.approver@banskabystrica.sk".into()),
        name: Some("Jana Approver".into()),
        roles: Vec::new(),
        groups: Vec::new(),
    };

    let refused = joinedcontext_portal::api::changes::approve_change_for(
        &state,
        &identity,
        "ovzdusie",
        "chg-00000042",
        Some("air"),
        joinedcontext_portal::api::changes::ApprovedBy::Operation,
    )
    .await
    .expect_err("a steward does not publish to the public");

    let said = refused.to_string();
    assert!(said.contains("publisher"), "{said}");
    assert!(said.contains("EP-76"), "{said}");
    assert!(!merged_66(&server).await);
}

/// T-0861: the approval walks every file of the merge request, so the detail lists every file
/// of the merge request. An approver who reads "create Pipeline aq" and is then refused for a
/// RoleBinding was never shown what the refusal is about.
#[tokio::test]
async fn the_detail_of_a_bundle_lists_every_file_with_its_kind_and_lane() {
    let (_server, state) = bundle_of(
        json!([{ "kinds": ["Pipeline", "RoleBinding"], "verbs": ["read", "approve"] }]),
        &[("users/assignments/mallory-admin.yaml", SMUGGLED_BINDING)],
    )
    .await;

    let config = state.config.clone();
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/changes/chg-00000007")
                .header(
                    header::COOKIE,
                    session_and_csrf_cookies(
                        &config,
                        "jana.approver",
                        Some("jana.approver@banskabystrica.sk"),
                        Some("Jana Approver"),
                        vec![],
                    ),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let proposal: Value = serde_json::from_slice(&bytes).expect("a change proposal");

    assert_eq!(proposal["fileCount"], json!(2));
    let files = proposal["files"]
        .as_array()
        .expect("the files of the bundle");
    let kinds: Vec<&str> = files
        .iter()
        .filter_map(|file| file["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"Pipeline"), "{files:?}");
    assert!(kinds.contains(&"RoleBinding"), "{files:?}");

    let binding = files
        .iter()
        .find(|file| file["kind"] == "RoleBinding")
        .expect("the binding is listed");
    assert_eq!(
        binding["path"],
        json!("users/assignments/mallory-admin.yaml")
    );
    assert_eq!(binding["operation"], json!("Create"));
    assert_eq!(
        binding["lane"],
        json!("red"),
        "a binding is the lane the confirmation is asked for"
    );
}
