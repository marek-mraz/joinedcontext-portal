use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::api::dry_run::DryRunResult;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::Lane;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::MockServer;

const TEST_CSRF_TOKEN: &str = "test-csrf-dry-run-token";

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
async fn post_dry_run_all_returns_200_and_makes_zero_git_requests() {
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
                .uri("/api/v1/projects/ovzdusie/spaces?dryRun=All")
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

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let res: DryRunResult = serde_json::from_slice(&body_bytes).expect("deserialize DryRunResult");
    assert!(res.valid);
    assert_eq!(res.lane, Lane::Green);
    assert_eq!(res.plan.summary.create, 1);
    assert_eq!(res.plan.summary.update, 0);
    assert_eq!(res.plan.summary.delete, 0);

    let requests = server.received_requests().await.expect("received requests");
    assert!(requests.is_empty());
}

#[tokio::test]
async fn put_dry_run_all_returns_200_and_makes_zero_git_requests() {
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
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility?dryRun=All")
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

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let res: DryRunResult = serde_json::from_slice(&body_bytes).expect("deserialize DryRunResult");
    assert!(res.valid);
    assert_eq!(res.lane, Lane::Green);
    assert_eq!(res.plan.summary.create, 0);
    assert_eq!(res.plan.summary.update, 1);
    assert_eq!(res.plan.summary.delete, 0);

    let requests = server.received_requests().await.expect("received requests");
    assert!(requests.is_empty());
}

#[tokio::test]
async fn delete_dry_run_all_returns_200_and_makes_zero_git_requests() {
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
                .uri("/api/v1/projects/ovzdusie/spaces/mobility?dryRun=All")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let res: DryRunResult = serde_json::from_slice(&body_bytes).expect("deserialize DryRunResult");
    assert!(res.valid);
    assert_eq!(res.lane, Lane::Red);
    assert_eq!(res.plan.summary.create, 0);
    assert_eq!(res.plan.summary.update, 0);
    assert_eq!(res.plan.summary.delete, 1);

    let requests = server.received_requests().await.expect("received requests");
    assert!(requests.is_empty());
}

#[tokio::test]
async fn invalid_dry_run_query_value_returns_400() {
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

    // 1. POST with ?dryRun=Foo
    let resp_post = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces?dryRun=Foo")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_post.status(), StatusCode::BAD_REQUEST);
    let bytes_post = resp_post
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_post: ProblemDetails = serde_json::from_slice(&bytes_post).expect("ProblemDetails");
    assert_eq!(problem_post.status, 400);
    assert_eq!(
        problem_post.r#type,
        "https://joinedcontext.com/errors/invalid-request"
    );
    assert!(problem_post
        .detail
        .as_deref()
        .expect("detail")
        .contains("invalid dryRun value 'Foo'; 'All' is the only accepted value"));

    // 2. PUT with ?dryRun=Foo
    let resp_put = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility?dryRun=Foo")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_put.status(), StatusCode::BAD_REQUEST);
    let bytes_put = resp_put
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_put: ProblemDetails = serde_json::from_slice(&bytes_put).expect("ProblemDetails");
    assert_eq!(problem_put.status, 400);
    assert!(problem_put
        .detail
        .as_deref()
        .expect("detail")
        .contains("invalid dryRun value 'Foo'; 'All' is the only accepted value"));

    // 3. DELETE with ?dryRun=Foo
    let resp_del = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility?dryRun=Foo")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_del.status(), StatusCode::BAD_REQUEST);
    let bytes_del = resp_del
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_del: ProblemDetails = serde_json::from_slice(&bytes_del).expect("ProblemDetails");
    assert_eq!(problem_del.status, 400);
    assert!(problem_del
        .detail
        .as_deref()
        .expect("detail")
        .contains("invalid dryRun value 'Foo'; 'All' is the only accepted value"));
}
