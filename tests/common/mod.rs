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
use joinedcontext_portal::store::Mirror;

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

/// A request the way every door makes it since T-0956 (PF-57): a proposal of a manifest — POST,
/// PUT or PATCH on a resource route, or a `*_propose` operation carrying a `manifest` — is checked
/// first with the same body (`?dryRun=All`, or `jc_manifest_dry_run`), and then sent. Any other
/// request is sent as it is; an approval is never repeated as a check.
pub async fn checked_send(
    state: &AppState,
    identity: Identity,
    http: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    if matches!(http, "POST" | "PUT" | "PATCH") && !uri.contains("dryRun") {
        let path = uri.split('?').next().unwrap_or(uri);
        let rest: Vec<&str> = path
            .strip_prefix("/api/v1/projects/")
            .map(|rest| rest.split('/').collect())
            .unwrap_or_default();
        match rest.as_slice() {
            [project, "ops", op] if op.ends_with("_propose") => {
                if let Some(manifest) = body.as_ref().and_then(|b| b.get("manifest")) {
                    let check = serde_json::json!({ "manifest": manifest });
                    let uri = format!("/api/v1/projects/{project}/ops/jc_manifest_dry_run");
                    send(state, identity.clone(), "POST", &uri, Some(check)).await;
                }
            }
            [_, plural] | [_, plural, _]
                if joinedcontext_portal::resource::by_plural(plural).is_some() =>
            {
                let joiner = if uri.contains('?') { '&' } else { '?' };
                let dry = format!("{uri}{joiner}dryRun=All");
                send(state, identity.clone(), http, &dry, body.clone()).await;
            }
            _ => {}
        }
    }
    send(state, identity, http, uri, body).await
}

/// A mirror holding the resources a manifest under test points at (MF-13, T-2233).
///
/// The dry run of every door refuses a manifest naming a resource that is not there, so a test
/// proposing one has to say what exists — the same thing the person's project says. Each pair is
/// `(kind, name)`, all in `namespace`.
pub fn mirror_holding(namespace: &str, resources: &[(&str, &str)]) -> Arc<Mirror> {
    let mirror = Mirror::new();
    for (kind, name) in resources {
        mirror.upsert(joinedcontext_portal::resource::ResourceEnvelope {
            api_version: joinedcontext_portal::resource::API_VERSION.to_owned(),
            kind: (*kind).to_owned(),
            metadata: joinedcontext_portal::resource::ObjectMeta {
                name: (*name).to_owned(),
                namespace: Some(namespace.to_owned()),
                ..Default::default()
            },
            spec: serde_json::json!({}),
            status: None,
        });
    }
    Arc::new(mirror)
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
    // Every file of one change goes in one commit to `POST /contents`, which is the repository
    // path itself and not a path under it, so the per-file mocks above never match it (T-0900).
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/contents")))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "commit-1" } })),
        )
        .mount(&gitea)
        .await;
    // One open change per resource: a removal asks the forge for the open merge requests first
    // (CC-34, T-0883). This fixture has none open — at the fallback priority, so a suite that
    // mounts a forge with an open change of its own is answered its own, not this empty list.
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .with_priority(9)
        .mount(&gitea)
        .await;
    // A removal lists the tree to find the files the resource owns beside its manifest (T-0900).
    // This fixture holds manifests alone, so the listing is empty and a delete removes the one
    // file; a suite with side files mounts a tree of its own, and this one stands at the fallback
    // priority so that tree is the one answered.
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/git/trees/.*")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tree": [], "truncated": false })),
        )
        .with_priority(9)
        .mount(&gitea)
        .await;
    gitea
}

/// A proposal the way every door makes one since T-0956: the same request as a dry run first, which
/// records a green verdict for the manifest (PF-57), then the proposal itself.
pub trait CheckFirst {
    async fn oneshot_checked(
        self,
        request: Request<Body>,
    ) -> Result<axum::response::Response, std::convert::Infallible>;
}

impl CheckFirst for axum::Router {
    async fn oneshot_checked(
        self,
        request: Request<Body>,
    ) -> Result<axum::response::Response, std::convert::Infallible> {
        let (parts, body) = request.into_parts();
        let bytes = body.collect().await.expect("request body").to_bytes();
        let uri = parts.uri.to_string();
        let dry_uri = if uri.contains('?') {
            format!("{uri}&dryRun=All")
        } else {
            format!("{uri}?dryRun=All")
        };
        let mut check = Request::builder().method(parts.method.clone()).uri(dry_uri);
        for (name, value) in &parts.headers {
            check = check.header(name, value);
        }
        let checked = self
            .clone()
            .oneshot(
                check
                    .body(Body::from(bytes.clone()))
                    .expect("check request"),
            )
            .await?;
        assert_eq!(
            checked.status(),
            StatusCode::OK,
            "the check before the proposal"
        );
        self.oneshot(Request::from_parts(parts, Body::from(bytes)))
            .await
    }
}
