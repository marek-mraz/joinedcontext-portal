use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use joinedcontext_portal::api::mutate::branch_name;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::{Change, ChangePhase, Lane, Operation};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;

use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
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
async fn create_returns_202_with_change_and_commits_to_gitea() {
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

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "not found"
        })))
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-created" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 42,
            "html_url": "https://gitea.example.sk/pulls/42",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
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
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert_eq!(
        change.status.merge_request.as_deref(),
        Some("https://gitea.example.sk/pulls/42")
    );
    assert_eq!(change.status.lane, Lane::Green);

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
        .starts_with("portal/create-contextspace-mobility-"));

    let put_req = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "PUT"
                && r.url
                    .path()
                    .starts_with("/api/v1/repos/test-owner/test-repo/contents/")
        })
        .expect("PUT contents request");
    let put_body: serde_json::Value =
        serde_json::from_slice(&put_req.body).expect("PUT request body");
    assert_eq!(put_body["author"]["name"], "Demo Steward");

    let content_b64 = put_body["content"].as_str().expect("base64 content");
    let decoded_bytes = STANDARD.decode(content_b64).expect("valid base64 content");
    let yaml_str = String::from_utf8(decoded_bytes).expect("valid UTF-8 YAML string");
    assert!(!yaml_str.contains("status:"));

    assert!(requests.iter().any(|r| {
        r.method.as_str() == "POST" && r.url.path() == "/api/v1/repos/test-owner/test-repo/pulls"
    }));
}

#[tokio::test]
async fn create_invalid_manifest_bodies_return_400() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    // 1. Foreign namespace
    let foreign_ns_body = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "foreign-project"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let resp_foreign = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&foreign_ns_body).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_foreign.status(), StatusCode::BAD_REQUEST);
    let bytes_foreign = resp_foreign
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_foreign: ProblemDetails =
        serde_json::from_slice(&bytes_foreign).expect("ProblemDetails");
    assert_eq!(problem_foreign.status, 400);
    assert!(problem_foreign
        .detail
        .as_deref()
        .expect("detail")
        .contains("foreign-project"));

    // 2. Status block in body (MF-04)
    let with_status_body = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        },
        "status": {
            "phase": "Live"
        }
    });

    let resp_status = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&with_status_body).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_status.status(), StatusCode::BAD_REQUEST);
    let bytes_status = resp_status
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_status: ProblemDetails =
        serde_json::from_slice(&bytes_status).expect("ProblemDetails");
    assert_eq!(problem_status.status, 400);
    assert!(problem_status
        .detail
        .as_deref()
        .expect("detail")
        .contains("status is computed by the platform"));

    // 3. Literal token string (MF-24)
    let with_token_body = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "auth": {
                "token": "ghp_super_secret_token_literal"
            }
        }
    });

    let resp_token = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&with_token_body).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_token.status(), StatusCode::BAD_REQUEST);
    let bytes_token = resp_token
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_token: ProblemDetails =
        serde_json::from_slice(&bytes_token).expect("ProblemDetails");
    assert_eq!(problem_token.status, 400);
    assert!(problem_token
        .detail
        .as_deref()
        .expect("detail")
        .contains("literal secret in field 'token' is forbidden"));
}

/// T-0412: the kind's own invariants run at write time. An inline Bloblang mapping on a
/// `mapping` step is what jc-core refuses (PL-41); the Portal refuses it too, before a branch.
#[tokio::test]
async fn create_with_a_spec_the_kind_refuses_returns_400_naming_the_field() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None));
    let body = json!({
        "apiVersion": API_VERSION,
        "kind": "Pipeline",
        "metadata": { "name": "aq-derived", "namespace": "ovzdusie" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt-mesto" } },
            "compute": { "kind": "mapping", "bloblang": "root = this" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air"
        }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/pipelines")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("Pipeline") && detail.contains("bloblang"),
        "{detail}"
    );
}

#[tokio::test]
async fn create_with_unknown_plural_returns_404() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {}
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/unknownplurals")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
    assert!(problem
        .detail
        .as_deref()
        .expect("detail")
        .contains("unknownplurals"));
}

#[tokio::test]
async fn replace_with_name_mismatch_returns_400() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "body-space-name",
            "namespace": "ovzdusie"
        },
        "spec": {}
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/spaces/path-space-name")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 400);
    assert!(problem
        .detail
        .as_deref()
        .expect("detail")
        .contains("metadata.name 'body-space-name' does not match path 'path-space-name'"));
}

#[tokio::test]
async fn patch_unsupported_media_type_returns_415() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let patch_payload = json!({
        "spec": {
            "isSandbox": false
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&patch_payload).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 415);
    assert!(problem
        .detail
        .as_deref()
        .expect("detail")
        .contains("application/merge-patch+json"));
}

#[tokio::test]
async fn patch_merge_patch_json_changes_field_and_returns_202() {
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

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-space-old",
            "content": "YXBpVmVyc2lvbjogeW91"
        })))
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "commit": { "sha": "commit-sha-patched" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 101,
            "html_url": "https://gitea.example.sk/pulls/101",
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

    let patch_payload = json!({
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/merge-patch+json")
                .body(Body::from(
                    serde_json::to_vec(&patch_payload).expect("json"),
                ))
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
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert_eq!(change.status.plan.update, 1);
    assert_eq!(
        change.status.merge_request.as_deref(),
        Some("https://gitea.example.sk/pulls/101")
    );
}

#[tokio::test]
async fn lane_classification_public_endpoint_red_and_sandbox_space_green() {
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

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/endpoints/public-air.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "not found"
        })))
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/endpoints/public-air.yaml",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-endpoint" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 88,
            "html_url": "https://gitea.example.sk/pulls/88",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    // 1. Public Endpoint -> Red Lane
    let public_endpoint = json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": {
            "name": "public-air",
            "namespace": "ovzdusie",
            "labels": {
                "joinedcontext.com/space": "mobility"
            }
        },
        "spec": {
            "contextSpaceRef": "mobility",
            "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }
    });

    let resp_endpoint = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/endpoints")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&public_endpoint).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_endpoint.status(), StatusCode::ACCEPTED);
    let bytes_ep = resp_endpoint
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change_ep: Change = serde_json::from_slice(&bytes_ep).expect("deserialize Change");
    assert_eq!(change_ep.status.lane, Lane::Red);
}

#[tokio::test]
async fn create_without_forge_returns_503() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
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
    assert!(problem
        .detail
        .as_deref()
        .expect("detail")
        .contains("git forge is not configured"));
}

#[tokio::test]
async fn create_without_csrf_header_returns_403() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, make_session_cookie(&config))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
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

/// A second proposal for a resource whose change is still open (T-0883, CC-34, PF-52): refused
/// before a byte reaches the branch, the open change named, nothing of the forge's own words.
#[tokio::test]
async fn a_second_proposal_while_a_change_is_open_names_it_and_writes_nothing() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    let open_branch = branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Create);
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "number": 175,
                "html_url": "https://gitea.example.sk/pulls/175",
                "state": "open",
                "title": "create ContextSpace mobility",
                "head": { "ref": open_branch },
                "base": { "ref": "main" },
                "created_at": "2026-09-15T20:00:00Z",
                "user": { "login": "jana.kovacova", "full_name": "Jana Kováčová" },
                "mergeable": true,
                "merged": false
            }
        ])))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&body).expect("problem");
    let detail = problem.detail.expect("detail");
    assert_eq!(
        detail,
        "a change for ContextSpace 'mobility' is already open: chg-000000af; approve or reject it first"
    );
    assert!(!detail.contains("pull request"), "no forge words: {detail}");

    let requests = server.received_requests().await.expect("received requests");
    assert!(
        !requests.iter().any(|r| r.method.as_str() != "GET"),
        "nothing was written to the forge"
    );
}

/// A change that a retry opened on a suffixed branch (T-0887) is the same resource: the second
/// proposal is refused and names it.
#[tokio::test]
async fn an_open_change_on_a_suffixed_branch_still_blocks_a_proposal() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    let open_branch = format!(
        "{}_0badf00d",
        branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Create)
    );
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "number": 176,
                "html_url": "https://gitea.example.sk/pulls/176",
                "state": "open",
                "title": "create ContextSpace mobility",
                "head": { "ref": open_branch },
                "base": { "ref": "main" },
                "created_at": "2026-09-15T20:00:00Z",
                "user": { "login": "jana.kovacova", "full_name": "Jana Kováčová" },
                "mergeable": true,
                "merged": false
            }
        ])))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&body).expect("problem");
    assert_eq!(
        problem.detail.expect("detail"),
        "a change for ContextSpace 'mobility' is already open: chg-000000b0; approve or reject it first"
    );
    let requests = server.received_requests().await.expect("received requests");
    assert!(
        !requests.iter().any(|r| r.method.as_str() != "GET"),
        "nothing was written to the forge"
    );
}

/// An open change on another resource is nobody's business here: the proposal goes through.
#[tokio::test]
async fn an_open_change_elsewhere_does_not_block_a_proposal() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "number": 9,
                "html_url": "https://gitea.example.sk/pulls/9",
                "state": "open",
                "head": { "ref": branch_name("ovzdusie", "ContextSpace", "parking", Operation::Create) },
                "base": { "ref": "main" },
                "merged": false
            }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 43,
            "html_url": "https://gitea.example.sk/pulls/43",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

/// A proposal's branch left by an earlier attempt is recreated from main (T-0886), so what the
/// approver reviews is this proposal alone.
#[tokio::test]
async fn a_stale_proposal_branch_is_recreated_from_main() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
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
    let branch = branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Create);
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/branches/{branch}"
        )))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 44,
            "html_url": "https://gitea.example.sk/pulls/44",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
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

#[tokio::test]
async fn a_kind_the_platform_cannot_load_is_refused_before_anything_is_written() {
    // T-0833: `Entity` is declared in Architecture/06 and jc-core does not define it: a seed is a
    // plain NGSI-LD `.json`, never a manifest. Every loader refuses an unknown kind and refuses
    // the whole repository with it, so one such file committed here would stop configuration
    // reaching every endpoint. `Subscription` used to be the other one, until T-0913.
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None));
    let body = json!({
        "apiVersion": API_VERSION,
        "kind": "Entity",
        "metadata": { "name": "air-alerts", "namespace": "ovzdusie" },
        "spec": { "type": "AirQualityObserved" }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/entities")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("Entity") && detail.contains("not defined"),
        "{detail}"
    );
}

/// T-0913: the kind the Portal used to refuse is written like any other now, and the file lands
/// in the space its `contextSpaceRef` names rather than in a folder named after the project.
#[tokio::test]
async fn a_subscription_is_proposed_into_the_space_it_watches() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    let file = "projects/ovzdusie/spaces/air-quality/subscriptions/air-alerts.yaml";

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/contents/{file}"
        )))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/contents/{file}"
        )))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-created" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 7,
            "html_url": "https://gitea.example.sk/pulls/7",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "Subscription",
        "metadata": { "name": "air-alerts", "namespace": "ovzdusie" },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "air-quality" },
            "entities": [{ "type": "AirQualityObserved" }],
            "notification": {
                "endpoint": { "uri": "https://alerts.example.fi/hooks/air-quality" }
            }
        }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/subscriptions")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("received requests");
    assert!(
        requests
            .iter()
            .any(|r| r.method.as_str() == "PUT" && r.url.path().ends_with(file)),
        "the subscription is written to {file}"
    );
}

/// T-0913, MF-31: the platform calls the notification address, so a credential written into it
/// is refused at the form rather than committed and found by whoever reads the repository.
#[tokio::test]
async fn a_subscription_carrying_a_literal_credential_is_refused() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None));
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "Subscription",
        "metadata": { "name": "air-alerts", "namespace": "ovzdusie" },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "air-quality" },
            "entities": [{ "type": "AirQualityObserved" }],
            "notification": {
                "endpoint": {
                    "uri": "https://alerts.example.fi/hooks/air-quality?token=s3cr3t-value"
                }
            }
        }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/subscriptions")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("secretRef"), "{detail}");
    assert!(
        !detail.contains("s3cr3t-value"),
        "the refusal repeated the secret: {detail}"
    );
}

/// PF-73, PF-74: the write that would put the project over a quota is refused before a change
/// exists, and the refusal names the count and the limit. The dry run says the same, so a person
/// learns it from the form and not from the merge request.
#[tokio::test]
async fn the_pipeline_above_the_quota_is_refused_with_the_count_and_the_limit() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let org = |spec: serde_json::Value| ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Organization".into(),
        metadata: ObjectMeta::new("bb", joinedcontext_portal::permissions::ORG_NAMESPACE),
        spec,
        status: None,
    };
    state.mirror.upsert(org(
        json!({ "domain": "banskabystrica.sk", "projects": { "quota": { "residentPipelines": 1 } } }),
    ));
    let pipeline = |name: &str| {
        json!({
            "apiVersion": API_VERSION,
            "kind": "Pipeline",
            "metadata": { "name": name, "namespace": "ovzdusie" },
            "spec": {
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt-mesto" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air"
            }
        })
    };
    state
        .mirror
        .upsert(serde_json::from_value::<ResourceEnvelope>(pipeline("first")).expect("a pipeline"));

    let post = |uri: &str, body: serde_json::Value| {
        let app = server::app(state.clone());
        let config = config.clone();
        let uri = uri.to_owned();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header(header::COOKIE, session_and_csrf_cookies(&config))
                        .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&body).expect("json")))
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
            (status, String::from_utf8_lossy(&bytes).into_owned())
        }
    };

    let (status, body) = post("/api/v1/projects/ovzdusie/pipelines", pipeline("second")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.contains("residentPipelines 2 of 1"),
        "the refusal names the count and the limit: {body}"
    );

    // The same answer on the door a form uses, before anything is proposed.
    let (status, body) = post(
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        pipeline("second"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("residentPipelines 2 of 1"), "{body}");

    // The project's own quota is what counts, and it may be raised there.
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Project".into(),
        metadata: ObjectMeta::new("ovzdusie", joinedcontext_portal::permissions::ORG_NAMESPACE),
        spec: json!({ "organizationRef": { "name": "bb" }, "quotas": { "residentPipelines": 5 } }),
        status: None,
    });
    let (status, body) = post(
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        pipeline("second"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// PF-73: a `Project` that raises a quota above the organization's default is a red-lane change,
/// so an org-admin decides it and not the project's own steward.
#[tokio::test]
async fn a_project_that_raises_its_quota_is_a_red_lane_change() {
    use joinedcontext_portal::change::classify;

    let spec = json!({ "organizationRef": { "name": "bb" }, "quotas": { "residentPipelines": 9 } });
    assert_eq!(classify("Project", Operation::Create, &spec), Lane::Red);
    assert_eq!(classify("Project", Operation::Update, &spec), Lane::Red);
}
