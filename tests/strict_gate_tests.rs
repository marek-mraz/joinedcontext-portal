//! Tests for strict validation gating and Verdict-based change proposal enforcement (AG-62, UI-48, PF-57).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::MockServer;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::ops::verdict::{Finding, Level, Verdict};
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const TEST_CSRF_TOKEN: &str = "test-csrf-token-strict-456";

fn session_cookie(
    config: &Config,
    username: &str,
    email: Option<&str>,
    roles: Vec<&str>,
    groups: Vec<&str>,
) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            subject: format!("sub-{username}"),
            username: username.to_string(),
            email: email.map(str::to_string),
            name: Some(username.to_string()),
            roles: roles.into_iter().map(str::to_string).collect(),
            groups: groups.into_iter().map(str::to_string).collect(),
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

async fn setup_mock_gitea() -> (MockServer, GiteaClient) {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client = GiteaClient::new(base_url, "owner", "repo", "token").unwrap();

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/repos/owner/repo"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path(
            "/api/v1/repos/owner/repo/branches",
        ))
        .respond_with(wiremock::ResponseTemplate::new(201).set_body_json(json!({
            "name": "portal/create-datasource-feed-strict-12345678"
        })))
        .mount(&server)
        .await;

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(wiremock::ResponseTemplate::new(404))
        .mount(&server)
        .await;

    wiremock::Mock::given(wiremock::matchers::method("PUT"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(wiremock::ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-created" }
        })))
        .mount(&server)
        .await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/api/v1/repos/owner/repo/pulls"))
        .respond_with(wiremock::ResponseTemplate::new(201).set_body_json(json!({
            "number": 42,
            "html_url": "https://git.example/pulls/42",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    (server, client)
}

#[tokio::test]
async fn strict_mode_propose_without_verdict_refused_409() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-strict", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-strict",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state);
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let payload = json!({
        "draft": {
            "kind": "DataSource",
            "name": "feed-strict"
        }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_datasource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["check"], "jc_datasource_check");
}

#[tokio::test]
async fn strict_mode_check_runs_and_attaches_verdict_then_propose_succeeds() {
    let (_gitea_server, gitea_client) = setup_mock_gitea().await;

    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror)
        .with_gitea(Arc::new(gitea_client));

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-strict", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-strict",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state.clone());
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let check_payload = json!({
        "draft": {
            "kind": "DataSource",
            "name": "feed-strict"
        }
    });

    let resp_check = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_datasource_check")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&check_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_check.status(), StatusCode::OK);
    let check_bytes = resp_check.into_body().collect().await.unwrap().to_bytes();
    let check_body: Value = serde_json::from_slice(&check_bytes).unwrap();
    assert_eq!(check_body["verdict"]["ok"], true);

    let draft_after_check = state
        .drafts
        .get("ovzdusie", "DataSource", "feed-strict")
        .await
        .unwrap()
        .expect("draft exists");
    assert!(draft_after_check.verdict.is_some());
    assert!(draft_after_check.verdict.as_ref().unwrap().ok);

    let propose_payload = json!({
        "draft": {
            "kind": "DataSource",
            "name": "feed-strict"
        }
    });

    let resp_propose = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_datasource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&propose_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_propose.status(), StatusCode::ACCEPTED);
    let prop_bytes = resp_propose.into_body().collect().await.unwrap().to_bytes();
    let prop_body: Value = serde_json::from_slice(&prop_bytes).unwrap();
    assert!(prop_body.get("changeId").is_some() || prop_body.get("change").is_some());

    assert!(state
        .drafts
        .get("ovzdusie", "DataSource", "feed-strict")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn strict_mode_stale_verdict_refused_409() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-stale", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-stale",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state.clone());
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let check_payload = json!({
        "draft": { "kind": "DataSource", "name": "feed-stale" }
    });
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_datasource_check")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&check_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let updated_manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-stale", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/updated_bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-stale",
            updated_manifest,
            Some(1),
            "steward",
            "person",
        )
        .await
        .unwrap();

    let propose_payload = json!({
        "draft": { "kind": "DataSource", "name": "feed-stale" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_datasource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&propose_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["reason"], "stale");
}

#[tokio::test]
async fn strict_mode_red_verdict_refused_409() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-red", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-red",
            manifest.clone(),
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let red_verdict = Verdict::red(
        &manifest,
        vec![Finding {
            level: Level::Error,
            path: "/spec/http/url".into(),
            message: "feed unreachable".into(),
        }],
        None,
    );
    state
        .drafts
        .set_verdict("ovzdusie", "DataSource", "feed-red", red_verdict)
        .await
        .unwrap();

    let app = server::app(state);
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let propose_payload = json!({
        "draft": { "kind": "DataSource", "name": "feed-red" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_datasource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&propose_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["reason"], "verdict_failed");
}

#[tokio::test]
async fn lax_mode_allows_propose_with_warning() {
    let (_gitea_server, gitea_client) = setup_mock_gitea().await;

    let branding_dir = std::env::temp_dir();
    let branding_path = branding_dir.join(format!("jc-branding-lax-{}.yaml", std::process::id()));
    std::fs::write(&branding_path, "validation: lax\n").expect("write branding file");

    let mut config = Config::for_tests();
    config.branding_file = Some(branding_path.to_string_lossy().to_string());

    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror)
        .with_gitea(Arc::new(gitea_client));

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed-lax", "namespace": "ovzdusie" },
        "spec": { "type": "http", "http": { "url": "https://example.com/bikes.json" } }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "DataSource",
            "feed-lax",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state.clone());
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let propose_payload = json!({
        "draft": { "kind": "DataSource", "name": "feed-lax" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_datasource_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&propose_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let _ = std::fs::remove_file(&branding_path);

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["warning"], "proposed without a fresh green verdict");
    assert!(state
        .drafts
        .get("ovzdusie", "DataSource", "feed-lax")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn pipeline_propose_requires_jc_pipeline_test() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "Pipeline",
        "metadata": { "name": "pipe-strict", "namespace": "ovzdusie" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "source-1" } },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:org:ovzdusie:all"
        }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "Pipeline",
            "pipe-strict",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state);
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let payload = json!({
        "draft": { "kind": "Pipeline", "name": "pipe-strict" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_pipeline_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["check"], "jc_pipeline_test");
}

#[tokio::test]
async fn space_propose_requires_jc_manifest_dry_run() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);

    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "space-strict", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });

    state
        .drafts
        .put(
            "ovzdusie",
            "ContextSpace",
            "space-strict",
            manifest,
            None,
            "steward",
            "person",
        )
        .await
        .unwrap();

    let app = server::app(state);
    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let payload = json!({
        "draft": { "kind": "ContextSpace", "name": "space-strict" }
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_space_propose")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "verdict_required");
    assert_eq!(body["check"], "jc_manifest_dry_run");
}
