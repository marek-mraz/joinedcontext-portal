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
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ResourceEnvelope, API_VERSION};
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
      - urn:ngsi-ld:AirQualityObserved:hel.fi:ovzdusie:sever-01
      - urn:ngsi-ld:AirQualityObserved:tampere.fi:ilma:keskusta-01
    idPattern: "^urn:ngsi-ld:AirQualityObserved:hel\\.fi:ovzdusie:.*$"
"#;

const ROLE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Role
metadata:
  name: air-steward
  namespace: org
spec:
  rules:
    - kinds: [Endpoint]
      verbs: [read]
"#;

const PROJECT_MANIFEST: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Project
metadata:
  name: helsinki
  namespace: org
spec:
  displayName: { en: Helsinki }
  organizationRef: hel
"#;

const BENTO: &str = "input:\n  mqtt:\n    urls: [ mqtts://mqtt.hsl.fi:8883 ]\n";

const BUNDLE: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: Bundle
metadata:
  name: helsinki
  namespace: org
spec:
  exportedAt: "2026-09-06T16:30:00Z"
  exportedBy: aino.virtanen
  sourceRevision: 8c56954a1f0e2b3c4d5e6f708192a3b4c5d6e7f8
  items:
    - { kind: ContextSpace, namespace: helsinki, name: ovzdusie, path: projects/helsinki/spaces/ovzdusie/space.yaml }
    - { kind: Endpoint, namespace: helsinki, name: public-air, path: projects/helsinki/endpoints/public-air.yaml }
  nativeFiles:
    - projects/helsinki/pipelines/aq/bento.yaml
  omitted: 0
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
        ("bundle.yaml", BUNDLE),
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

fn session_cookie(config: &Config, groups: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            subject: "f:1:jana".into(),
            username: "jana.kovacova".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
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
fn cookies(config: &Config, groups: &[&str]) -> String {
    format!("{}; {CSRF_COOKIE}={CSRF}", session_cookie(config, groups))
}

/// The bootstrap administrator of `Config::for_tests`, who may propose every kind.
fn state(server: &MockServer, existing: Vec<&str>) -> (AppState, String) {
    state_as(server, existing, &["portal-approver"], vec![])
}

/// A person in no group, whose rights are exactly the organization's `Role`s and
/// `RoleBinding`s in `org` (T-0798, PF-50).
fn state_as(
    server: &MockServer,
    existing: Vec<&str>,
    groups: &[&str],
    org: Vec<Value>,
) -> (AppState, String) {
    let config = Config::for_tests();
    let cookie = cookies(&config, groups);
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
    // The organization the import lands in: its domain is the one every imported id is
    // rewritten to (PF-10, PF-43).
    mirror.upsert(
        serde_json::from_value(json!({
            "apiVersion": API_VERSION,
            "kind": "Organization",
            "metadata": { "name": "bb", "namespace": ORG_NAMESPACE },
            "spec": { "displayName": { "en": "Banska Bystrica" }, "domain": "banskabystrica.sk" },
        }))
        .expect("organization manifest"),
    );
    for manifest in org {
        mirror.upsert(serde_json::from_value(manifest).expect("organization manifest"));
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

/// A `Role` of the organization repository with the given rules.
fn role(name: &str, rules: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Role",
        "metadata": { "name": name, "namespace": ORG_NAMESPACE },
        "spec": { "rules": rules },
    })
}

/// The signed-in person holds `role` on this project.
fn binding(role: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "RoleBinding",
        "metadata": { "name": format!("jana-{role}"), "namespace": ORG_NAMESPACE },
        "spec": {
            "subjects": [{ "user": "jana.kovacova" }],
            "role": role,
            "scope": { "project": PROJECT },
        },
    })
}

/// Everything the forge saw, as `METHOD path`.
async fn forge_calls(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
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
    // MF-22, PF-43: the organisation segment of an id moves to the organisation that now owns
    // the space, or the gateway refuses every write the imported endpoint selects. The space
    // segment is a ContextSpace name and keeps it.
    assert!(
        endpoint.contains("urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:sever-01"),
        "{endpoint}"
    );
    // An id in a space this bundle does not carry is another city's, and a federated
    // registration that pointed there still does (MF-22).
    assert!(
        endpoint.contains("urn:ngsi-ld:AirQualityObserved:tampere.fi:ilma:keskusta-01"),
        "{endpoint}"
    );
    // An anchored idPattern is a URN prefix with the domain's dots escaped (R33, T-0826).
    assert!(
        endpoint.contains(r"^urn:ngsi-ld:AirQualityObserved:banskabystrica\.sk:ovzdusie:.*$"),
        "{endpoint}"
    );
    // The one place the source survives is the provenance annotation (MF-20).
    assert!(
        !endpoint.contains(&format!("namespace: {SOURCE}")),
        "{endpoint}"
    );
    assert!(!endpoint.contains("hel.fi"), "{endpoint}");
}

#[tokio::test]
async fn an_organization_scoped_kind_lands_in_org_and_the_source_project_manifest_is_dropped() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let bundle = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("users/roles/air-steward.yaml", ROLE),
        ("projects/helsinki/project.yaml", PROJECT_MANIFEST),
        ("bundle.yaml", BUNDLE),
    ]);
    let (content_type, body) = multipart(&bundle, &[("conflictPolicy", "fail")]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");

    let paths = written(&server).await;
    // PF-22: an organization-scoped kind is stored at `users/roles/{name}.yaml`, namespace
    // `org`; jc-core refuses it under a project's namespace.
    assert!(
        paths.iter().any(|p| p == "users/roles/air-steward.yaml"),
        "{paths:?}"
    );
    // The destination project is the one in the URL, so the bundle's own Project manifest
    // never writes a `projects/helsinki/` directory into this repository.
    assert!(
        !paths.iter().any(|p| p.starts_with("projects/helsinki/")),
        "{paths:?}"
    );
    let role = put_bodies(&server)
        .await
        .into_iter()
        .find(|body| body.contains("kind: Role"))
        .expect("the role was written");
    assert!(role.contains("namespace: org"), "{role}");
    assert!(!role.contains(&format!("namespace: {PROJECT}")), "{role}");
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
async fn the_readme_and_schemas_of_a_complete_export_are_never_written_into_the_project() {
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let complete = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/endpoints/public-air.yaml", ENDPOINT),
        ("projects/helsinki/pipelines/aq/bento.yaml", BENTO),
        ("bundle.yaml", BUNDLE),
        ("README.md", "# Project helsinki\n"),
        (
            "schemas/kinds/Endpoint.schema.json",
            "{\"type\": \"object\"}",
        ),
        (
            "schemas/models/air/air.linkml.yaml",
            "id: https://example.org/air\n",
        ),
    ]);
    let (content_type, body) = multipart(&complete, &[("dryRun", "true")]);
    let (status, report) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["created"], json!(["ovzdusie", "public-air"]));
    assert_eq!(
        report["nativeFiles"], 1,
        "only bento.yaml is the project's own native file (MF-41): {report}"
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

// --- who may import (T-0798, PF-50) -----------------------------------------------------

#[tokio::test]
async fn an_import_by_someone_with_no_binding_is_refused_before_anything_is_planned() {
    let server = forge().await;
    for fields in [&[][..], &[("dryRun", "All")][..]] {
        let (state, cookie) = state_as(&server, vec![], &[], vec![]);
        let (content_type, body) = multipart(&bundle_archive(), fields);
        let (status, problem) = post(state, &cookie, &content_type, body).await;

        assert_eq!(status, StatusCode::FORBIDDEN, "{fields:?}: {problem}");
        let detail = problem["detail"].as_str().unwrap_or_default();
        assert!(detail.contains("PF-50"), "{detail}");
    }
    // Not a branch, not a file, not a merge request: the forge was never asked.
    let calls = forge_calls(&server).await;
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("POST") || c.starts_with("PUT")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn a_role_that_grants_propose_on_every_kind_in_the_bundle_imports_it() {
    let server = forge().await;
    let importer = role(
        "importer",
        json!([{ "kinds": ["ContextSpace", "Endpoint", "Pipeline"], "verbs": ["propose"] }]),
    );
    let (state, cookie) = state_as(&server, vec![], &[], vec![importer, binding("importer")]);
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let (status, change) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::ACCEPTED, "{change}");
    assert_eq!(change["status"]["plan"]["create"], 2);
    assert!(written(&server)
        .await
        .iter()
        .any(|p| p == "projects/banskabystrica/pipelines/aq/bento.yaml"));
}

#[tokio::test]
async fn a_role_that_misses_one_kind_of_the_bundle_refuses_the_whole_bundle_naming_it() {
    let server = forge().await;
    // ContextSpace and Endpoint, but not the Pipeline whose bento.yaml the bundle carries.
    let partial = role(
        "partial",
        json!([{ "kinds": ["ContextSpace", "Endpoint"], "verbs": ["propose"] }]),
    );
    let (state, cookie) = state_as(&server, vec![], &[], vec![partial, binding("partial")]);
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let (status, problem) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("Pipeline"), "{detail}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_constraint_the_bundles_manifest_violates_refuses_it_naming_the_field() {
    let server = forge().await;
    // The bundle's Endpoint is public; this role may propose internal ones only.
    let internal = role(
        "internal-editor",
        json!([
            { "kinds": ["ContextSpace", "Pipeline"], "verbs": ["propose"] },
            { "kinds": ["Endpoint"], "verbs": ["propose"],
              "constraints": [{ "field": "spec.audience", "notIn": ["public"] }] }
        ]),
    );
    let (state, cookie) = state_as(
        &server,
        vec![],
        &[],
        vec![internal, binding("internal-editor")],
    );
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let (status, problem) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("spec.audience"), "{detail}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_binding_on_another_project_grants_nothing_here() {
    let server = forge().await;
    let importer = role(
        "importer",
        json!([{ "kinds": ["ContextSpace", "Endpoint", "Pipeline"], "verbs": ["propose"] }]),
    );
    let mut elsewhere = binding("importer");
    elsewhere["spec"]["scope"] = json!({ "project": SOURCE });
    let (state, cookie) = state_as(&server, vec![], &[], vec![importer, elsewhere]);
    let (content_type, body) = multipart(&bundle_archive(), &[]);
    let (status, problem) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_native_file_under_a_directory_that_names_no_kind_is_refused_even_with_every_role() {
    let server = forge().await;
    // Every kind the catalogue has, and still no role can cover `projects/x/notes/`.
    let kinds: Vec<&str> = joinedcontext_portal::resource::kinds()
        .map(|info| info.kind)
        .collect();
    let everything = role(
        "everything",
        json!([{ "kinds": kinds, "verbs": ["propose"] }]),
    );
    let (state, cookie) = state_as(
        &server,
        vec![],
        &[],
        vec![everything, binding("everything")],
    );
    let file = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/notes/todo.txt", "remember the milk\n"),
    ]);
    let (content_type, body) = multipart(&file, &[]);
    let (status, problem) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("notes/todo.txt"), "{detail}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_bundle_carrying_a_built_digest_is_refused_whole() {
    // AP-11, AP-13a (T-0822): an App that names an image is an App that deploys it. The image
    // a bundle carries was built somewhere else, and this cluster runs what it built itself.
    const APP: &str = r#"apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata:
  name: air-map
  namespace: helsinki
  annotations:
    joinedcontext.com/image: "ghcr.io/hel/air-map@sha256:9f2b1c0d4e5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e"
spec:
  endpointRefs: [public-air]
  visibility: project
"#;
    let server = forge().await;
    let (state, cookie) = state(&server, vec![]);
    let bundle = archive(&[
        ("projects/helsinki/spaces/ovzdusie/space.yaml", SPACE),
        ("projects/helsinki/apps/air-map.yaml", APP),
    ]);
    let (content_type, body) = multipart(&bundle, &[("conflictPolicy", "fail")]);
    let (status, answer) = post(state, &cookie, &content_type, body).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert!(
        answer["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("joinedcontext.com/image"),
        "{answer}"
    );
    // Nothing of the bundle was written: a refusal is whole (MF-24).
    assert!(written(&server).await.is_empty());
}
