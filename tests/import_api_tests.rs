//! Importing a bundle into a project against a mocked forge (T-0193, MF-20…MF-26).
//!
//! An import is the one write that touches many resources at once, so what these tests check
//! is what an operator cannot see by reading the diff: that the bundle was refused whole when
//! any part of it was wrong, that the conflict policy did what it says, and that a rename
//! carried every reference with it. A bundle that half-imports is worse than one that does
//! not, because the half that landed is now a project nobody described.

use std::io::Write;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::ResourceEnvelope;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request as MockRequest, ResponseTemplate};

const PROJECT: &str = "banskabystrica";
const SOURCE: &str = "helsinki";
const CSRF: &str = "test-csrf-token-12345";

const SPACE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: ovzdusie
  namespace: helsinki
spec:
  displayName: { en: Air quality }
"#;

const ENDPOINT: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata:
  name: public-air
  namespace: helsinki
spec:
  contextSpaceRef: ovzdusie
  slug: mluyob4nz52lok3ssk7pgn5vwt
  audience: public
  entitySelector:
    ids:
      - urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:sever-01
"#;

const BENTO: &str = "input:\n  mqtt:\n    urls: [ mqtts://mqtt.hsl.fi:8883 ]\n";

const BUNDLE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Bundle
metadata:
  name: helsinki
  namespace: helsinki
spec:
  project: helsinki
  revision: 8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8
  exporter: aino.virtanen
  files: 3
  omitted: 0
  contents: []
"#;

fn archive(files: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in files {
        writer.start_file(*name, options).expect("archive entry");
        writer.write_all(content.as_bytes()).expect("archive write");
    }
    writer.finish().expect("archive closes").into_inner()
}

fn bundle_archive() -> Vec<u8> {
    archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/endpoints/public-air.yaml", ENDPOINT),
        ("projects/helsinki/pipelines/aq/bento.yaml", BENTO),
        ("projects/helsinki/bundle.yaml", BUNDLE),
    ])
}

/// A multipart body with one file part and the wizard's option fields.
fn multipart(file: &[u8], fields: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = "jcimportboundary";
    let mut body: Vec<u8> = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"bundle.zip\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn forge() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/bb/org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/bb/org/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "name": "portal/import" })))
        .mount(&server)
        .await;
    // Nothing is on the import branch yet, so every write is a create.
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v1/repos/bb/org/contents/.*$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r"^/api/v1/repos/bb/org/contents/.*$"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "content": { "sha": "b1ob" },
            "commit": { "sha": "c0mm1t" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/bb/org/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 412,
            "html_url": "https://git.example.sk/bb/org/pulls/412",
            "title": "import",
            "state": "open",
            "head": { "ref": "portal/import" },
            "base": { "ref": "main" }
        })))
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

/// The session and the double-submit CSRF cookie, which every UI mutation carries.
fn cookies(config: &Config) -> String {
    format!("{}; {CSRF_COOKIE}={CSRF}", session_cookie(config))
}

fn state(server: &MockServer, existing: Vec<&str>) -> (AppState, String) {
    let config = Config::for_tests();
    let cookie = cookies(&config);
    let gitea = Arc::new(
        GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "bb",
            "org",
            "token",
        )
        .expect("gitea client"),
    );
    let mirror = Arc::new(Mirror::new());
    for yaml in existing {
        let mut envelope: ResourceEnvelope = serde_yaml_ng::from_str(yaml).expect("manifest");
        envelope.metadata.namespace = Some(PROJECT.to_string());
        mirror.upsert(envelope);
    }
    let state = AppState::new(config, None)
        .with_gitea(gitea)
        .with_mirror(mirror);
    (state, cookie)
}

async fn post(
    state: AppState,
    cookie: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let app = server::app(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/projects/{PROJECT}/import"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, content_type)
                .header(CSRF_HEADER, CSRF)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("the request completes");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn upload(
    server: &MockServer,
    existing: Vec<&str>,
    fields: &[(&str, &str)],
) -> (StatusCode, Value) {
    let (state, cookie) = state(server, existing);
    let (content_type, body) = multipart(&bundle_archive(), fields);
    post(state, &cookie, &content_type, body).await
}

async fn written(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request: &MockRequest| request.method.as_str() == "PUT")
        .map(|request| {
            request
                .url
                .path()
                .replace("/api/v1/repos/bb/org/contents/", "")
        })
        .collect()
}

async fn put_bodies(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request: &MockRequest| request.method.as_str() == "PUT")
        .filter_map(|request| {
            let payload: Value = serde_json::from_slice(&request.body).ok()?;
            let encoded = payload.get("content")?.as_str()?.to_owned();
            String::from_utf8(STANDARD.decode(encoded).ok()?).ok()
        })
        .collect()
}

// --- the happy path ---------------------------------------------------------------------

#[tokio::test]
async fn an_archive_becomes_one_merge_request_for_the_whole_bundle() {
    let server = forge().await;
    let (status, body) = upload(&server, vec![], &[("conflictPolicy", "fail")]).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["kind"], "Change");
    assert_eq!(
        body["status"]["mergeRequest"],
        "https://git.example.sk/bb/org/pulls/412"
    );
    // MF-21, CC-63: one merge request, one lane, one diff for the bundle.
    assert_eq!(body["status"]["plan"]["create"], 2);

    let paths = written(&server).await;
    // An Endpoint lives under the space it serves, and the space segment travels with it.
    assert!(
        paths
            .iter()
            .any(|p| p == "projects/banskabystrica/spaces/ovzdusie/endpoints/public-air.yaml"),
        "{paths:?}"
    );
    // The native file travels beside its manifest, reprojected into this project.
    assert!(
        paths
            .iter()
            .any(|p| p == "projects/banskabystrica/pipelines/aq/bento.yaml"),
        "{paths:?}"
    );
    // The bundle index describes the bundle and is not a resource of the project (MF-17).
    assert!(
        !paths.iter().any(|p| p.ends_with("bundle.yaml")),
        "{paths:?}"
    );
}

#[tokio::test]
async fn every_imported_manifest_says_where_it_came_from() {
    let server = forge().await;
    upload(&server, vec![], &[]).await;

    let manifests: Vec<String> = put_bodies(&server)
        .await
        .into_iter()
        .filter(|body| body.contains("kind: Endpoint"))
        .collect();
    assert_eq!(manifests.len(), 1);
    // MF-20: provenance travels with the object, and the bundle index is what names the source.
    assert!(
        manifests[0].contains("joinedcontext.com/imported-from")
            && manifests[0].contains("helsinki@8c56954"),
        "{}",
        manifests[0]
    );
}

#[tokio::test]
async fn the_namespace_the_typed_references_and_the_urns_all_move() {
    let server = forge().await;
    upload(&server, vec![], &[]).await;

    let endpoint = put_bodies(&server)
        .await
        .into_iter()
        .find(|body| body.contains("kind: Endpoint"))
        .expect("the endpoint was written");
    assert!(
        endpoint.contains(&format!("namespace: {PROJECT}")),
        "{endpoint}"
    );
    // MF-22: the space segment of an entity URN moves with the namespace, or the imported
    // endpoint keeps selecting entities of the project it came from.
    assert!(
        endpoint.contains(&format!(
            "urn:ngsi-ld:AirQualityObserved:hel.fi:{PROJECT}:sever-01"
        )),
        "{endpoint}"
    );
    // The one place the source survives is the provenance annotation (MF-20).
    assert!(
        !endpoint.contains(&format!("namespace: {SOURCE}")),
        "{endpoint}"
    );
    assert!(!endpoint.contains(&format!(":{SOURCE}:")), "{endpoint}");
}

#[tokio::test]
async fn a_dry_run_reports_what_it_would_do_and_writes_nothing() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let (content_type, body) = multipart(&bundle_archive(), &[("dryRun", "true")]);
    let (status, report) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["created"], json!(["ovzdusie", "public-air"]));
    assert_eq!(report["nativeFiles"], 1);
    assert_eq!(
        report["source"],
        "helsinki@8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8"
    );
    assert!(
        written(&server).await.is_empty(),
        "a dry run must not write"
    );
}

#[tokio::test]
async fn a_manifest_posted_as_json_is_imported_too() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let manifest: Value = serde_yaml_ng::from_str(SPACE).expect("the space parses");
    let body = json!({ "manifests": { "apiVersion": "joinedcontext.com/v1alpha1",
                                      "kind": "List", "items": [manifest] } });
    let (status, answer) = post(
        state,
        &cookie,
        "application/json",
        serde_json::to_vec(&body).expect("body"),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    assert_eq!(written(&server).await.len(), 1);
}

// --- the conflict policy (MF-23) ---------------------------------------------------------

#[tokio::test]
async fn fail_refuses_the_whole_bundle_when_one_resource_already_exists() {
    let server = forge().await;
    let (status, body) = upload(&server, vec![ENDPOINT], &[("conflictPolicy", "fail")]).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("public-air"),
        "{body}"
    );
    // The refusal is whole: the space that had no conflict must not have landed either.
    assert!(
        written(&server).await.is_empty(),
        "a refused import must write nothing"
    );
}

#[tokio::test]
async fn skip_leaves_the_existing_resource_alone_and_imports_the_rest() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![ENDPOINT]);
    let (content_type, body) = multipart(
        &bundle_archive(),
        &[("conflictPolicy", "skip"), ("dryRun", "true")],
    );
    let (status, report) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["skipped"], json!(["public-air"]));
    assert_eq!(report["created"], json!(["ovzdusie"]));
}

#[tokio::test]
async fn replace_overwrites_the_existing_resource() {
    let server = forge().await;
    let (status, body) = upload(&server, vec![ENDPOINT], &[("conflictPolicy", "replace")]).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["status"]["plan"]["update"], 1);
    assert!(written(&server)
        .await
        .iter()
        .any(|p| p.contains("public-air.yaml")));
}

#[tokio::test]
async fn rename_imports_under_a_new_name_and_carries_the_references_with_it() {
    let server = forge().await;
    // The space is the one that collides, and the endpoint points at it by name.
    let (state, cookie) = state(&server, vec![SPACE]);
    let (content_type, body) = multipart(&bundle_archive(), &[("conflictPolicy", "rename")]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    let endpoint = put_bodies(&server)
        .await
        .into_iter()
        .find(|written| written.contains("kind: Endpoint"))
        .expect("the endpoint was written");
    // MF-26: a rename that does not move the references produces a bundle that imports and
    // then dangles, which is the failure mode project duplication exists to avoid.
    assert!(
        endpoint.contains("contextSpaceRef: ovzdusie-helsinki"),
        "{endpoint}"
    );
}

// --- the gates (MF-24) --------------------------------------------------------------------

#[tokio::test]
async fn a_literal_secret_anywhere_in_the_bundle_refuses_it() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let leaky = ENDPOINT.replace(
        "  audience: public\n",
        "  audience: public\n  publish:\n    apiKey: jc_dead_beef\n",
    );
    let (content_type, body) = multipart(
        &archive(&[
            ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
            ("projects/helsinki/endpoints/public-air.yaml", &leaky),
        ]),
        &[],
    );
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("apiKey"),
        "{answer}"
    );
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_reference_to_a_resource_nobody_has_refuses_the_bundle() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    // The endpoint alone: the space it names is in neither the bundle nor the project.
    let (content_type, body) = multipart(
        &archive(&[("projects/helsinki/endpoints/public-air.yaml", ENDPOINT)]),
        &[],
    );
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    let detail = answer["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("ContextSpace 'ovzdusie'"), "{detail}");
}

#[tokio::test]
async fn a_reference_the_project_already_holds_resolves() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![SPACE]);
    let (content_type, body) = multipart(
        &archive(&[("projects/helsinki/endpoints/public-air.yaml", ENDPOINT)]),
        &[("conflictPolicy", "skip")],
    );
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
}

#[tokio::test]
async fn a_status_block_is_refused_because_the_platform_computes_it() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let with_status = format!("{ENDPOINT}status:\n  phase: Live\n");
    let (content_type, body) = multipart(
        &archive(&[
            ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
            ("projects/helsinki/endpoints/public-air.yaml", &with_status),
        ]),
        &[],
    );
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("MF-04"),
        "{answer}"
    );
}

#[tokio::test]
async fn a_kind_this_platform_does_not_serve_is_refused() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let alien = SPACE.replace("kind: ContextSpace", "kind: DeploymentConfig");
    let (content_type, body) = multipart(&archive(&[("projects/helsinki/x.yaml", &alien)]), &[]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("DeploymentConfig"),
        "{answer}"
    );
}

#[tokio::test]
async fn an_unsupported_api_version_is_refused() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let old = SPACE.replace("joinedcontext.com/v1alpha1", "joinedcontext.com/v1beta9");
    let (content_type, body) = multipart(&archive(&[("projects/helsinki/x.yaml", &old)]), &[]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("v1beta9"),
        "{answer}"
    );
}

#[tokio::test]
async fn an_archive_that_escapes_its_root_is_refused() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let (content_type, body) = multipart(&archive(&[("../../etc/passwd.yaml", SPACE)]), &[]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn importing_from_a_url_says_it_is_not_implemented_rather_than_fetching() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let body = json!({ "url": "http://169.254.169.254/latest/meta-data" });
    let (status, answer) = post(
        state,
        &cookie,
        "application/json",
        serde_json::to_vec(&body).expect("body"),
    )
    .await;

    // MF-20 allows a URL and the Portal has no egress policy for one; fetching whatever a
    // caller names would be inventing an SSRF surface, so it is refused in the open.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{answer}");
}

#[tokio::test]
async fn an_anonymous_import_is_refused_before_anything_is_parsed() {
    let server = forge().await;
    let (state, _) = state(&server, vec![]);
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let app = server::app(state);
    // A matching double-submit pair, so the CSRF guard lets the request through and the
    // missing session is what refuses it.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/projects/{PROJECT}/import"))
                .header(header::COOKIE, format!("{CSRF_COOKIE}={CSRF}"))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("the request completes");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(written(&server).await.is_empty());
}
