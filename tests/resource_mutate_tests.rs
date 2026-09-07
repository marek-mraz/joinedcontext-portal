use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
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
