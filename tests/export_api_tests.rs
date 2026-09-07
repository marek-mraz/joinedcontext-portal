//! One-click export against a mocked forge (T-0192, CC-49, MF-16…MF-19).
//!
//! Everything an export hands out comes from the repository at one revision, so the forge is the
//! only thing these tests have to fake. What they check is what must never travel: live status,
//! a literal secret, or a file from another project.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REVISION: &str = "8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8";

/// An Endpoint manifest with a status the Portal computed and a secret someone smuggled in.
const ENDPOINT: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: public-air
  namespace: banskabystrica
spec:
  contextSpaceRef: ovzdusie
  slug: mluyob4nz52lok3ssk7pgn5vwt
  audience: public
  publish:
    apiKey: "jc_dead_beef_secret"
status:
  phase: Live
  sourceUrl: https://git.example.sk/bb/org/src/branch/main/x.yaml
"#;

const PIPELINE: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Pipeline
metadata:
  name: aq-mqtt-ingest
  namespace: banskabystrica
spec:
  class: resident
  secretRefs:
    - name: mqtt-credentials
      key: password
"#;

/// A native file beside a manifest: Bento's own configuration, ours only to carry.
const BENTO: &str = "input:\n  mqtt:\n    urls: [ mqtts://mqtt.hsl.fi:8883 ]\n";

const OTHER_PROJECT: &str = r#"
apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: doprava-public
  namespace: bb-doprava
spec:
  slug: zt4qm7ge2xdv6ksb3ncf5arw2y
"#;

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "projects/banskabystrica/endpoints/public-air.yaml",
            ENDPOINT,
        ),
        (
            "projects/banskabystrica/pipelines/aq-mqtt-ingest/pipeline.yaml",
            PIPELINE,
        ),
        (
            "projects/banskabystrica/pipelines/aq-mqtt-ingest/bento.yaml",
            BENTO,
        ),
        (
            "projects/bb-doprava/endpoints/doprava-public.yaml",
            OTHER_PROJECT,
        ),
    ]
}

async fn forge() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/bb/org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/bb/org/branches/main"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": REVISION } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/repos/bb/org/git/trees/{REVISION}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tree": files()
                .iter()
                .map(|(path, _)| json!({ "path": path, "type": "blob" }))
                .collect::<Vec<_>>(),
            "truncated": false
        })))
        .mount(&server)
        .await;
    for (file_path, content) in files() {
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/repos/bb/org/contents/{file_path}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "b1ob",
                "content": STANDARD.encode(content),
            })))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/bb/org/commits"))
        .and(query_param("path", "projects/banskabystrica"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "sha": REVISION,
                "commit": {
                    "message": "Endpoint public-air: add csv\n\nlonger body",
                    "author": { "name": "Jana Kováčová", "date": "2026-09-06T16:30:00Z" }
                }
            }
        ])))
        .mount(&server)
        .await;
    server
}

fn session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            subject: "f:1:jana".into(),
            username: "jana.kovacova".into(),
            email: None,
            name: None,
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
    let jar = session::store(jar, &session).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

struct Answer {
    status: StatusCode,
    content_type: String,
    disposition: String,
    body: Vec<u8>,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

async fn get(uri: &str, with_forge: bool) -> Answer {
    let server = forge().await;
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let mut state = AppState::new(config, None);
    if with_forge {
        let client = GiteaClient::new(server.uri().parse().unwrap(), "bb", "org", "token")
            .expect("gitea client");
        state = state.with_gitea(Arc::new(client));
    }
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let header_of = |name: header::HeaderName| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header_of(header::CONTENT_TYPE);
    let disposition = header_of(header::CONTENT_DISPOSITION);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        content_type,
        disposition,
        body: body.to_vec(),
    }
}

#[tokio::test]
async fn anonymous_callers_never_reach_the_repository() {
    let response = server::app(AppState::new(Config::for_tests(), None))
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/banskabystrica/export")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn without_a_forge_there_is_nothing_to_export() {
    let answer = get("/api/v1/projects/banskabystrica/export", false).await;
    assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(answer.json()["status"], 503);
}

#[tokio::test]
async fn the_yaml_stream_carries_the_project_without_status_or_secrets() {
    let answer = get("/api/v1/projects/banskabystrica/export", true).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert!(answer.content_type.starts_with("application/yaml"));
    assert!(
        answer.disposition.contains("banskabystrica-8c56954.yaml"),
        "the download names the project and the revision: {}",
        answer.disposition
    );

    let body = answer.text();
    assert!(body.contains("name: public-air"));
    assert!(body.contains("name: aq-mqtt-ingest"));
    assert!(
        !body.contains("jc_dead_beef_secret"),
        "MF-17: no secret value leaves in a bundle"
    );
    assert!(
        !body.contains("phase: Live") && !body.contains("sourceUrl"),
        "MF-17: status is the Portal's computation and is stripped"
    );
    assert!(
        body.contains("key: password"),
        "a secretRef names a key and must survive, or the bundle is not re-importable"
    );
    assert!(
        !body.contains("doprava-public"),
        "another project's manifest is not part of this export"
    );
}

#[tokio::test]
async fn kinds_and_names_narrow_the_stream() {
    let only_endpoints = get(
        "/api/v1/projects/banskabystrica/export?kinds=endpoints",
        true,
    )
    .await
    .text();
    assert!(only_endpoints.contains("public-air"));
    assert!(!only_endpoints.contains("aq-mqtt-ingest"));

    let by_name = get(
        "/api/v1/projects/banskabystrica/export?kinds=pipelines&names=aq-mqtt-ingest",
        true,
    )
    .await
    .text();
    assert!(by_name.contains("aq-mqtt-ingest"));
    assert!(!by_name.contains("public-air"));

    let nothing = get(
        "/api/v1/projects/banskabystrica/export?names=does-not-exist",
        true,
    )
    .await;
    assert_eq!(
        nothing.status,
        StatusCode::OK,
        "a filter that matches nothing is an empty export, not a 404"
    );
    assert!(nothing.text().trim().is_empty());
}

#[tokio::test]
async fn the_json_format_is_a_list_of_the_same_manifests() {
    let answer = get(
        "/api/v1/projects/banskabystrica/export?format=json&kinds=endpoints",
        true,
    )
    .await;
    assert!(answer.content_type.starts_with("application/json"));
    let body = answer.json();
    assert_eq!(body["kind"], "List");
    assert_eq!(body["metadata"]["revision"], REVISION);
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["metadata"]["name"], "public-air");
    assert!(items[0]["status"].is_null(), "status is stripped");
    assert_eq!(items[0]["spec"]["publish"]["apiKey"], "");
}

#[tokio::test]
async fn the_archive_is_a_zip_with_the_native_files_and_a_bundle_index() {
    let answer = get("/api/v1/projects/banskabystrica/export?format=zip", true).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.content_type, "application/zip");
    assert!(answer.disposition.contains("banskabystrica-8c56954.zip"));
    assert_eq!(&answer.body[..2], b"PK", "a zip starts with its magic");

    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(answer.body.clone())).expect("a readable zip");
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    assert!(names.contains(&"projects/banskabystrica/endpoints/public-air.yaml".to_string()));
    assert!(
        names.contains(&"projects/banskabystrica/pipelines/aq-mqtt-ingest/bento.yaml".to_string()),
        "the native file travels with its manifest (MF-17)"
    );
    assert!(!names.iter().any(|name| name.contains("bb-doprava")));

    let mut content = String::new();
    {
        use std::io::Read;
        archive
            .by_name("projects/banskabystrica/bundle.yaml")
            .expect("the bundle index")
            .read_to_string(&mut content)
            .expect("readable");
    }
    assert!(content.contains("kind: Bundle"));
    assert!(content.contains(REVISION));
    assert!(content.contains("public-air"));
    assert!(content.contains("omitted: 0"));

    // Deflate would hide a plain substring search, so every entry is read out and checked.
    for index in 0..archive.len() {
        use std::io::Read;
        let mut entry = archive.by_index(index).expect("entry");
        let name = entry.name().to_string();
        let mut text = String::new();
        entry.read_to_string(&mut text).expect("utf-8 entry");
        assert!(
            !text.contains("jc_dead_beef_secret"),
            "a secret value survived into {name}"
        );
    }
}

#[tokio::test]
async fn a_format_or_revision_the_export_does_not_serve_is_refused() {
    let bad_format = get("/api/v1/projects/banskabystrica/export?format=tar.gz", true).await;
    assert_eq!(bad_format.status, StatusCode::BAD_REQUEST);

    let traversal = get(
        "/api/v1/projects/banskabystrica/export?revision=../../etc/passwd",
        true,
    )
    .await;
    assert_eq!(
        traversal.status,
        StatusCode::BAD_REQUEST,
        "a revision is a commit or a branch, never a path"
    );

    let bad_project = get("/api/v1/projects/Banska%20Bystrica/export", true).await;
    assert_eq!(bad_project.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_revision_picker_reads_the_history_of_this_project() {
    let answer = get("/api/v1/projects/banskabystrica/revisions", true).await;
    assert_eq!(answer.status, StatusCode::OK);
    let items = answer.json()["items"].as_array().expect("items").clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["sha"], REVISION);
    assert_eq!(
        items[0]["message"], "Endpoint public-air: add csv",
        "the picker shows the subject line, not the whole message"
    );
    assert_eq!(items[0]["author"], "Jana Kováčová");

    let too_many = get("/api/v1/projects/banskabystrica/revisions?limit=500", true).await;
    assert_eq!(too_many.status, StatusCode::BAD_REQUEST);
    let none = get("/api/v1/projects/banskabystrica/revisions?limit=0", true).await;
    assert_eq!(none.status, StatusCode::BAD_REQUEST);
}
