//! Signed-in requests against the Portal router and the forge fixtures the permission tests share.
//!
//! Each test binary that declares `mod common;` compiles the whole file, and none of them uses
//! all of it, so what one binary leaves unused is not dead code.
#![allow(dead_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

pub const CSRF: &str = "test-csrf-token-permissions";

/// The mock forge's repository path.
pub const REPO: &str = "/api/v1/repos/test-owner/test-repo";

/// A person signed in as `{name}@hel.fi`, with no group or realm role.
pub fn person(name: &str) -> Identity {
    Identity {
        subject: format!("sub-{name}"),
        username: name.to_owned(),
        email: Some(format!("{name}@hel.fi")),
        name: Some(name.to_owned()),
        roles: Vec::new(),
        groups: Vec::new(),
    }
}

pub fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, namespace),
        spec,
        status: None,
    }
}

/// A forge file's content as Gitea returns it.
pub fn encode(text: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(text)
}

pub fn cookie(config: &Config, identity: Identity) -> String {
    let now = session::now_unix();
    let session = Session {
        identity,
        expires_at: now + 3600,
        issued_at: now,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar =
        session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

pub struct Answer {
    pub status: StatusCode,
    pub text: String,
}

/// One request as `identity`, through the whole router: session cookie, CSRF and all.
pub async fn send(
    state: &AppState,
    identity: Identity,
    http: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let content_type = if http == "PATCH" {
        "application/merge-patch+json"
    } else {
        "application/json"
    };
    let config = state.config.clone();
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method(http)
                .uri(uri)
                .header(header::COOKIE, cookie(&config, identity))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, content_type)
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
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
    Answer {
        status,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// The Portal's state against the mock forge `gitea`, with an empty mirror.
pub fn state_on(gitea: &MockServer) -> AppState {
    let client = GiteaClient::new(
        gitea.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client))
}

/// A forge that takes every write: branches, files, merge requests, merges and closes. The files
/// and merge requests a test reads are mounted on top of it.
pub async fn forge() -> MockServer {
    let gitea = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/branches")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&gitea)
        .await;
    for verb in ["PUT", "POST", "DELETE"] {
        Mock::given(method(verb))
            .and(path_regex(format!("^{REPO}/contents/.*")))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({ "commit": { "sha": "commit-1" } })),
            )
            .mount(&gitea)
            .await;
    }
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 9, "html_url": "https://gitea.example/pulls/9", "state": "open",
            "mergeable": true, "merged": false
        })))
        .mount(&gitea)
        .await;
    Mock::given(path_regex(format!("^{REPO}/pulls/[0-9]+/(merge|reviews)$")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&gitea)
        .await;
    Mock::given(method("PATCH"))
        .and(path_regex(format!("^{REPO}/pulls/[0-9]+$")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "state": "closed" })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/contents/.*")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .with_priority(9)
        .mount(&gitea)
        .await;
    gitea
}
