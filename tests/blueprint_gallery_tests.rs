//! The gallery's data and the flow it starts (T-0207, T-0208, CC-24, CC-30, CC-59).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::CSRF_COOKIE;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

const TEST_CSRF_TOKEN: &str = "test-csrf-token-12345";

fn session_cookie(config: &Config, roles: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@banskabystrica.sk".into()),
            name: Some("Demo Steward".into()),
            roles: roles.iter().map(|r| (*r).to_string()).collect(),
        },
        expires_at: now + 3600,
        issued_at: now,
        id_token: "id-token-placeholder".into(),
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &session).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        parts.push(raw.split(';').next().unwrap_or_default().to_string());
    }
    format!("{}; {CSRF_COOKIE}={TEST_CSRF_TOKEN}", parts.join("; "))
}

fn blueprint(name: &str, category: &str, risk: &str, allowed_roles: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Blueprint".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            // Organization-level, like `org.yaml`: `sync.rs` files it under "org" whatever
            // its path says, and the gallery lists exactly that namespace.
            namespace: Some("org".to_string()),
            ..Default::default()
        },
        spec: json!({
            "version": "1.2.0",
            "category": category,
            "riskClass": risk,
            "allowedRoles": allowed_roles,
            "parameterSchema": { "type": "object", "properties": { "webhookUrl": { "type": "string" } } },
            "templates": [{ "name": "subscription", "template": "kind: Subscription" }],
        }),
        status: None,
    }
}

fn seeded_mirror() -> Arc<Mirror> {
    let mirror = Mirror::new();
    mirror.upsert(blueprint(
        "threshold-alert",
        "alerting",
        "green",
        json!(["domain-editor"]),
    ));
    mirror.upsert(blueprint(
        "cross-city-sharing",
        "federation",
        "red",
        json!(["org-admin"]),
    ));
    mirror.upsert(blueprint(
        "open-to-everyone",
        "onboarding",
        "yellow",
        json!([]),
    ));
    Arc::new(mirror)
}

fn app_with(roles: &[&str]) -> (axum::Router, String) {
    let config = Config::for_tests();
    let cookie = session_cookie(&config, roles);
    let app = server::app(AppState::new(config, None).with_mirror(seeded_mirror()));
    (app, cookie)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

fn names(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            item["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

async fn get_gallery(roles: &[&str]) -> Value {
    let (app, cookie) = app_with(roles);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/blueprints")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

async fn post_flow(roles: &[&str], body: Value) -> (StatusCode, Value) {
    let (app, cookie) = app_with(roles);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/flows")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", TEST_CSRF_TOKEN)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[tokio::test]
async fn the_gallery_shows_only_what_this_caller_may_run() {
    // CC-59: `spec.allowedRoles` is the filter, and a blueprint naming no role is open to
    // everyone who can reach the gallery at all.
    let list = get_gallery(&["domain-editor"]).await;
    assert_eq!(names(&list), vec!["open-to-everyone", "threshold-alert"]);

    let admin = get_gallery(&["org-admin"]).await;
    assert_eq!(
        names(&admin),
        vec!["cross-city-sharing", "open-to-everyone"]
    );
}

#[tokio::test]
async fn a_caller_with_no_role_still_sees_the_unrestricted_blueprints() {
    let list = get_gallery(&[]).await;
    assert_eq!(names(&list), vec!["open-to-everyone"]);
}

#[tokio::test]
async fn the_gallery_carries_what_a_card_needs_to_render() {
    let list = get_gallery(&["domain-editor"]).await;
    let card = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["metadata"]["name"] == "threshold-alert")
        .expect("the blueprint this caller may run");
    assert_eq!(card["spec"]["riskClass"], "green");
    assert_eq!(card["spec"]["category"], "alerting");
    assert_eq!(card["spec"]["version"], "1.2.0");
    // The form is generated from this and from nothing else (CC-24, CC-31).
    assert_eq!(card["spec"]["parameterSchema"]["type"], "object");
}

#[tokio::test]
async fn an_anonymous_call_is_refused_as_a_problem_document() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config, None).with_mirror(seeded_mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/blueprints")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/problem+json")
    );
}

#[tokio::test]
async fn running_a_blueprint_this_caller_may_not_see_answers_like_a_missing_one() {
    // Hiding a card is not an authorisation, and a different answer here would tell a caller
    // which blueprints exist (R20).
    let (status, _) = post_flow(
        &["domain-editor"],
        json!({ "blueprint": "cross-city-sharing", "version": "1.2.0", "parameters": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (missing, _) = post_flow(
        &["domain-editor"],
        json!({ "blueprint": "no-such-blueprint", "version": "1.2.0", "parameters": {} }),
    )
    .await;
    assert_eq!(missing, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_form_filled_against_another_version_is_a_conflict() {
    // CC-26: expanding these values against the current schema would produce a manifest the
    // person filling the form never saw.
    let (status, problem) = post_flow(
        &["domain-editor"],
        json!({ "blueprint": "threshold-alert", "version": "1.0.0", "parameters": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        problem["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("1.2.0"),
        "the answer must name the version that is current now: {problem}"
    );
}

#[tokio::test]
async fn a_build_without_the_expansion_engine_says_so_instead_of_rendering() {
    // CC-25: expansion is jcctl's, the one the reconciler runs. A second engine in the Portal
    // would render manifests that differ from what a re-render produces, so this build refuses.
    let (status, problem) = post_flow(
        &["domain-editor"],
        json!({ "blueprint": "threshold-alert", "version": "1.2.0", "parameters": { "webhookUrl": "https://example.org/hook" } }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(problem["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("expand"));
}

#[tokio::test]
async fn a_flow_without_a_session_never_reaches_the_blueprint() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config, None).with_mirror(seeded_mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/flows")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "blueprint": "open-to-everyone", "version": "1.2.0", "parameters": {} })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        response.status() == StatusCode::UNAUTHORIZED || response.status() == StatusCode::FORBIDDEN,
        "an unauthenticated flow must not be started: {}",
        response.status()
    );
}
