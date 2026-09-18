//! Tests for reading and saving DataModel LinkML source and compiled schema artifacts (DM-01, DM-02, DM-22, DM-24, DM-56).

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::{Change, ChangePhase, Lane};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_CSRF_TOKEN: &str = "csrf-token-12345";

const PUBLISHED_LINKML: &str = r#"id: https://example.org/models/air-quality
name: air-quality
prefixes:
  aq: https://example.org/aq/
default_prefix: aq
imports:
  - linkml:types
classes:
  AirQualityObserved:
    class_uri: https://example.org/aq/AirQualityObserved
    slots:
      - dateObserved
      - pm10
slots:
  dateObserved:
    range: string
    required: true
  pm10:
    range: integer
    required: false
"#;

const COMPILED_ARTIFACTS: &str = r##"{
  "jsonSchema": {"title": "AirQualityObserved"},
  "context": {"@context": {"pm10": "https://example.org/aq/pm10"}},
  "docs": "# AirQualityObserved Documentation",
  "example": {"id": "urn:ngsi-ld:AirQualityObserved:01", "type": "AirQualityObserved"},
  "generatorVersion": "linkml-1.11.1"
}"##;

fn session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("steward@banskabystrica.sk".into()),
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
    parts.push(format!("{CSRF_COOKIE}={TEST_CSRF_TOKEN}"));
    parts.join("; ")
}

fn seed_datamodel(state: &AppState, project: &str, name: &str, linkml_path: &str, version: &str) {
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some(project.to_string()),
            ..Default::default()
        },
        spec: json!({
            "contextSpaceRef": "mobility",
            "linkml": linkml_path,
            "version": version,
            "lifecycle": "published",
            "classes": ["AirQualityObserved"]
        }),
        status: None,
    });
}

#[tokio::test]
async fn get_source_answers_file_content_and_yaml_content_type() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/yaml; charset=utf-8"
    );
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert_eq!(body_str, PUBLISHED_LINKML);
}

#[tokio::test]
async fn path_traversal_and_absolute_paths_are_400_and_forge_is_not_called() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));

    seed_datamodel(
        &state,
        "ovzdusie",
        "escape-relative",
        "../secrets.linkml.yaml",
        "1.0.0",
    );
    seed_datamodel(
        &state,
        "ovzdusie",
        "escape-absolute",
        "/etc/passwd.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    for name in ["escape-relative", "escape-absolute"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/projects/ovzdusie/datamodels/{name}/source"
                    ))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    assert_eq!(forge.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn other_project_or_absent_model_is_404() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None);
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/other-project/datamodels/air-quality/source")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn anonymous_request_returns_401() {
    let config = Config::for_tests();
    let state = AppState::new(config, None);
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn put_breaking_change_under_same_major_is_400_naming_removed_slot() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    // Remove slot pm10 -> breaking change
    let breaking_source = r#"id: https://example.org/models/air-quality
name: air-quality
classes:
  AirQualityObserved:
    slots:
      - dateObserved
slots:
  dateObserved:
    range: string
    required: true
"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source?version=1.0.1")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(breaking_source))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("breaking change"), "{detail}");
    let errors = problem["errors"].as_array().unwrap();
    assert!(
        errors.iter().any(|e| e.as_str().unwrap().contains("pm10")),
        "errors must name removed slot pm10"
    );
}

#[tokio::test]
async fn put_source_that_does_not_compile_is_400_with_tool_messages() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"errors":["slot 'temperature' has no range declared"]}"#,
            "application/json",
        ))
        .mount(&tools)
        .await;

    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let candidate = r#"id: https://example.org/models/air-quality
name: air-quality
classes:
  AirQualityObserved:
    slots:
      - dateObserved
      - temperature
slots:
  dateObserved:
    range: string
  temperature: {}
"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(candidate))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("temperature"),
        "detail must contain tool error: {detail}"
    );
}

#[tokio::test]
async fn put_dry_run_all_returns_200_and_proposes_nothing() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(COMPILED_ARTIFACTS, "application/json"),
        )
        .mount(&tools)
        .await;

    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let additive_source = r#"id: https://example.org/models/air-quality
name: air-quality
classes:
  AirQualityObserved:
    slots:
      - dateObserved
      - pm10
      - co2
slots:
  dateObserved:
    range: string
    required: true
  pm10:
    range: integer
    required: false
  co2:
    range: float
    required: false
"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source?dryRun=All")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(additive_source))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let res: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(res["severity"], "additive");
    assert_eq!(res["version"], "1.1.0");
    assert_eq!(
        res["artifacts"]["docs"],
        "# AirQualityObserved Documentation"
    );

    // Proposes nothing
    let forge_reqs = forge.received_requests().await.unwrap();
    assert!(!forge_reqs
        .iter()
        .any(|r| r.method.as_str() == "POST" && r.url.path().contains("/pulls")));
}

#[tokio::test]
async fn put_creates_change_and_writes_manifest_source_and_four_artifacts() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&forge)
        .await;

    // Handle GET content and PUT content for files
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&forge)
        .await;

    Mock::given(method("PUT"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&forge)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 88,
            "html_url": "https://forge.example.sk/pulls/88",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&forge)
        .await;

    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(COMPILED_ARTIFACTS, "application/json"),
        )
        .mount(&tools)
        .await;

    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let additive_source = r#"id: https://example.org/models/air-quality
name: air-quality
classes:
  AirQualityObserved:
    slots:
      - dateObserved
      - pm10
      - co2
slots:
  dateObserved:
    range: string
    required: true
  pm10:
    range: integer
    required: false
  co2:
    range: float
    required: false
"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(additive_source))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let change: Change = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert_eq!(change.status.lane, Lane::Yellow);
    assert_eq!(
        change.status.merge_request.as_deref(),
        Some("https://forge.example.sk/pulls/88")
    );

    // Verify all 6 files written to the branch
    let requests = forge.received_requests().await.unwrap();
    let put_paths: Vec<String> = requests
        .iter()
        .filter(|r| {
            r.method.as_str() == "PUT"
                && r.url
                    .path()
                    .starts_with("/api/v1/repos/owner/repo/contents/")
        })
        .map(|r| r.url.path().to_string())
        .collect();

    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/air-quality.yaml")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/air-quality.linkml.yaml")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/json-schema/air-quality.v1.json")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/context/air-quality.v1.jsonld")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/docs/air-quality.md")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/examples/air-quality.example.jsonld")));
}

/// The space a created model belongs to; without it there is no folder to write into (DM-57).
fn seed_space(state: &AppState, project: &str, name: &str) {
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some(project.to_string()),
            ..Default::default()
        },
        spec: json!({ "visibility": "project" }),
        status: None,
    });
}

const NEW_MODEL: &str = r#"id: https://example.org/models/bikes
name: bikes
classes:
  BikeHireDockingStation:
    slots:
      - availableBikeNumber
slots:
  availableBikeNumber:
    range: integer
"#;

#[tokio::test]
async fn put_for_a_name_the_project_does_not_hold_creates_the_model_in_its_space() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&forge)
        .await;
    // Nothing of this model is in the repository yet.
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&forge)
        .await;
    Mock::given(method("PUT"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 91,
            "html_url": "https://forge.example.sk/pulls/91",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&forge)
        .await;

    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(COMPILED_ARTIFACTS, "application/json"),
        )
        .mount(&tools)
        .await;

    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_space(&state, "ovzdusie", "mobility");

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes/source?space=mobility")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(NEW_MODEL))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let change: Change = serde_json::from_slice(&body_bytes).unwrap();
    // A model that did not exist breaks nothing, so the lane is the one a draft gets.
    assert_eq!(change.status.lane, Lane::Green);
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert_eq!(change.status.plan.create, 6);

    let requests = forge.received_requests().await.unwrap();
    let put_paths: Vec<String> = requests
        .iter()
        .filter(|r| {
            r.method.as_str() == "PUT"
                && r.url
                    .path()
                    .starts_with("/api/v1/repos/owner/repo/contents/")
        })
        .map(|r| r.url.path().to_string())
        .collect();

    // The Change carries the manifest as well as the source, which is what makes the model exist.
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/projects/ovzdusie/spaces/mobility/datamodels/bikes.linkml.yaml")));
    // A new model starts at 0.1.0, so its artifacts are the v0 ones.
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/json-schema/bikes.v0.json")));
}

/// T-1139, DM-56: a source past the limit is refused by the route, before the body is read into
/// a generator call and before the forge is touched. The ceiling is the one the layer enforces,
/// so a caller learns it from the answer rather than from a closed connection.
#[tokio::test]
async fn put_of_a_source_past_the_limit_is_refused_and_the_forge_is_not_called() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_space(&state, "ovzdusie", "mobility");

    // One byte over: the refusal is the size, not the content.
    let huge = "#".repeat(512 * 1024 + 1);
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes/source?space=mobility")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(huge))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        response.status() == StatusCode::BAD_REQUEST
            || response.status() == StatusCode::PAYLOAD_TOO_LARGE,
        "a source past the limit is refused, not accepted: {}",
        response.status()
    );
    assert!(forge.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn put_for_an_unknown_model_without_a_space_is_400_and_the_forge_is_not_called() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_space(&state, "ovzdusie", "mobility");

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes/source")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(NEW_MODEL))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(forge.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn put_naming_a_space_the_project_does_not_hold_is_404() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None);

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes/source?space=nowhere")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(NEW_MODEL))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A Role in the organization that reads `kinds`, bound to `who@hel.fi` on the project `ovzdusie`.
fn grant_read(state: &AppState, who: &str, kinds: serde_json::Value) {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    state.mirror.upsert(common::envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "reader-ovzdusie",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": format!("{who}@hel.fi") }],
            "role": "reader",
            "scope": { "project": "ovzdusie" },
        }),
    ));
}

/// PF-59, R20 (T-1367): the source is a read of the DataModel, so a person who may not read the
/// project's models is answered as if the model were not there, and the forge is never asked.
#[tokio::test]
async fn get_source_without_read_on_datamodel_is_404_and_the_forge_is_not_asked() {
    let forge = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main",
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .expect(0)
        .mount(&forge)
        .await;
    let state = common::state_on(&forge);
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );
    grant_read(&state, "pipelines-only", json!(["Pipeline"]));
    let uri = "/api/v1/projects/ovzdusie/datamodels/air-quality/source";

    for who in ["stranger", "pipelines-only"] {
        let answer = common::send(&state, common::person(who), "GET", uri, None).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{who}: {}",
            answer.text
        );
        assert!(
            !answer.text.contains("AirQualityObserved"),
            "{who} read the source"
        );
        // The same answer as a model that does not exist, so the name discloses nothing.
        let absent = common::send(
            &state,
            common::person(who),
            "GET",
            "/api/v1/projects/ovzdusie/datamodels/no-such-model/source",
            None,
        )
        .await;
        assert_eq!(
            answer.text.replace("air-quality", "no-such-model"),
            absent.text,
            "{who}: the refusal differs from an absent model"
        );
    }
}

/// The refusal's other half: a person whose role reads DataModel gets the source.
#[tokio::test]
async fn get_source_with_read_on_datamodel_answers_the_source() {
    let forge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;
    let state = common::state_on(&forge);
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );
    grant_read(&state, "modeller", json!(["DataModel"]));

    let answer = common::send(
        &state,
        common::person("modeller"),
        "GET",
        "/api/v1/projects/ovzdusie/datamodels/air-quality/source",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(answer.text, PUBLISHED_LINKML);
}
