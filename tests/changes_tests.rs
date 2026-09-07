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
use serde_json::json;
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

    let space_mobility = r#"apiVersion: joinedcontext.com/v1alpha1
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
    assert_eq!(list.items[1].author.name, "Jana Kováčová");
    assert_eq!(
        list.items[1].author.email.as_deref(),
        Some("jana.kovacova@banskabystrica.sk")
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

    // Mock saw review with REQUEST_CHANGES and zero merges
    let requests = server.received_requests().await.expect("received requests");
    let reviews: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/reviews"))
        .collect();
    assert_eq!(reviews.len(), 1);
    let review_body: serde_json::Value =
        serde_json::from_slice(&reviews[0].body).expect("review json body");
    assert_eq!(review_body["event"], "REQUEST_CHANGES");
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

    assert_eq!(reviews.len(), 1);
    assert_eq!(merges.len(), 1);

    let review_body: serde_json::Value = serde_json::from_slice(&reviews[0].body).unwrap();
    assert_eq!(review_body["event"], "APPROVED");

    let merge_body: serde_json::Value = serde_json::from_slice(&merges[0].body).unwrap();
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

    // Mock saw review with REQUEST_CHANGES and zero merges
    let requests = server.received_requests().await.expect("received requests");
    let reviews: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/reviews"))
        .collect();
    assert_eq!(reviews.len(), 1);

    let review_body: serde_json::Value = serde_json::from_slice(&reviews[0].body).unwrap();
    assert_eq!(review_body["event"], "REQUEST_CHANGES");
    assert_eq!(review_body["body"], "Change proposal rejected");

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
