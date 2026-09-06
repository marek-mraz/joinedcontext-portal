//! T-0181: Keycloak OIDC authorization code flow with PKCE and encrypted cookie sessions.
//!
//! The realm is a wiremock stand-in: discovery is enough to exercise the flow start,
//! the state check and the failure paths without a live Keycloak.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Shaped like a Keycloak realm: the issuer carries the `/realms/{realm}` path.
const REALM_PATH: &str = "/realms/banskabystrica";

fn issuer_of(server: &MockServer) -> String {
    format!("{}{REALM_PATH}", server.uri().trim_end_matches('/'))
}

async fn realm() -> MockServer {
    let server = MockServer::start().await;
    let issuer = issuer_of(&server);
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM_PATH}/.well-known/openid-configuration"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "end_session_endpoint": format!("{issuer}/protocol/openid-connect/logout"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/certs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [] })))
        .mount(&server)
        .await;
    server
}

async fn app_with_realm(server: &MockServer) -> axum::Router {
    let mut config = Config::from_vars(|k| match k {
        "JC_OIDC_ISSUER" => Some(issuer_of(server)),
        "JC_OIDC_CLIENT_ID" => Some("joinedcontext-portal".to_string()),
        "JC_OIDC_CLIENT_SECRET" => Some("test-secret".to_string()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        _ => None,
    })
    .expect("config");
    config.public_base_url = "https://portal.test".parse().expect("url");
    let state = AppState::from_config(config).await.expect("discovery");
    server::app(state)
}

fn set_cookie_values(response: &axum::response::Response) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn login_starts_a_pkce_flow_and_parks_the_verifier_in_a_secure_cookie() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(location.contains("code_challenge="), "{location}");
    assert!(
        location.contains("code_challenge_method=S256"),
        "{location}"
    );
    assert!(location.contains("state="), "{location}");
    assert!(location.contains("nonce="), "{location}");
    assert!(location.contains("scope=openid"), "{location}");
    assert!(
        location.contains("redirect_uri=https%3A%2F%2Fportal.test%2Fapi%2Fv1%2Fauth%2Fcallback"),
        "{location}"
    );

    let cookies = set_cookie_values(&response);
    let flow = cookies
        .iter()
        .find(|c| c.starts_with("jc_oidc_flow="))
        .expect("flow cookie");
    assert!(flow.contains("HttpOnly"), "{flow}");
    assert!(flow.contains("Secure"), "{flow}");
    assert!(flow.contains("SameSite=Lax"), "{flow}");
}

#[tokio::test]
async fn callback_without_a_login_in_flight_is_refused() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?code=abc&state=xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn callback_with_a_foreign_state_is_refused() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let started = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let flow_cookie = set_cookie_values(&started)
        .into_iter()
        .find(|c| c.starts_with("jc_oidc_flow="))
        .map(|c| c.split(';').next().unwrap_or_default().to_string())
        .expect("flow cookie");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?code=abc&state=not-the-state-we-issued")
                .header(header::COOKIE, flow_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem["type"],
        "https://joinedcontext.com/errors/invalid-request"
    );
    assert!(problem["detail"].as_str().unwrap().contains("state"));
}

#[tokio::test]
async fn callback_reports_a_provider_side_error() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?error=access_denied&error_description=user%20said%20no")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn me_without_a_session_is_unauthorized() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn login_without_a_configured_realm_is_unavailable() {
    let app = server::app(AppState::new(Config::for_tests(), None));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn a_mutation_without_the_csrf_token_is_forbidden() {
    let app = server::app(AppState::new(Config::for_tests(), None));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn a_mutation_with_a_matching_csrf_token_passes_the_gate() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let token = "double-submit-token";

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::COOKIE, format!("jc_csrf={token}"))
                .header("x-csrf-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "logout answers once CSRF passes"
    );
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let target: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(
        target["endSessionUrl"]
            .as_str()
            .is_some_and(|u| !u.is_empty()),
        "the SPA needs somewhere to navigate: {target}"
    );
}
