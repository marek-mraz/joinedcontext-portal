use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;

fn make_session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
            roles: Vec::new(),
        },
        expires_at: now + 3600,
        issued_at: now,
        id_token: "id-token-placeholder".into(),
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn manifest(project: &str, kind: &str, name: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(project.into()),
            ..Default::default()
        },
        spec: serde_json::json!({}),
        status: None,
    }
}

/// What the dev repository holds: `projects/banskabystrica/` and `projects/helsinki/`.
fn two_project_mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(manifest("helsinki", "ContextSpace", "helsinki"));
    mirror.upsert(manifest("helsinki", "Endpoint", "helsinki-bikes"));
    mirror.upsert(manifest("banskabystrica", "ContextSpace", "ovzdusie"));
    mirror.upsert(manifest("banskabystrica", "Endpoint", "public-air"));
    mirror
}

#[tokio::test]
async fn anonymous_call_returns_401_and_discloses_nothing() {
    let app =
        server::app(AppState::new(Config::for_tests(), None).with_mirror(two_project_mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&body).contains("helsinki"));
}

#[tokio::test]
async fn lists_every_project_of_the_mirror_once_sorted() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let app = server::app(AppState::new(config, None).with_mirror(two_project_mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["kind"], "List");
    assert_eq!(list["apiVersion"], API_VERSION);
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["banskabystrica", "helsinki"]);
}

#[tokio::test]
async fn empty_repository_lists_no_project() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let app = server::app(AppState::new(config, None).with_mirror(Arc::new(Mirror::new())));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 0);
}
