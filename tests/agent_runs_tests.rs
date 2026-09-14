//! Builder runs through the real router (T-0538, AG-43…AG-46, AG-52, AP-42, AP-44, AP-46).
//!
//! Every case drives the routes a person or the credential proxy actually calls. Four
//! properties are the reason the file exists: a public application is refused before anything
//! is scheduled, a run may not ask for more than its endpoint publishes, a cancelled run's
//! ticket stops working, and the two internal routes are reachable with the proxy's token and
//! with nothing else.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::agents::reaper;
use joinedcontext_portal::agents::run::{digest_prompt, mint_run_id, mint_ticket, AgentRun};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

const CSRF: &str = "csrf-token-value";
const PROXY_TOKEN: &str = "the-token-only-jc-agent-proxy-has";
const PROJECT: &str = "helsinki";
const STEWARD: &str = "demo.steward";
const SLUG: &str = "si6epqkx364lprho5uaigutk274r5grb";

/// A Portal with an agent runner configured and no cluster to schedule in: the shape every case
/// here runs against, because the workspace is not what these routes are about.
fn config() -> Config {
    Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some("http://jc-agent-proxy.agents.svc.cluster.local:8080"),
            "JC_AGENT_PROXY_TOKEN" => Some(PROXY_TOKEN),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete")
}

fn session_cookie(config: &Config, username: &str, roles: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: Some(format!("{username}@hel.fi")),
            name: None,
            roles: roles.iter().map(|role| role.to_string()).collect(),
            groups: Vec::new(),
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
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect();
    parts.push(format!("jc_csrf={CSRF}"));
    parts.join("; ")
}

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

fn builder_profile_spec() -> Value {
    json!({
        "role": "builder",
        "runtime": {
            "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
            "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
        },
        "model": { "provider": "anthropic", "name": "claude-sonnet-5", "maxTokensPerRun": 400000 },
        "limits": {
            "stepsPerRun": 120,
            "wallClock": "PT20M",
            "concurrentRunsPerOrganization": 2,
            "requestsPerMinute": 60,
            "maxResponseBytes": 2097152
        },
        "egress": { "allowedHosts": ["registry.npmjs.org", "static.crates.io"] },
        "tools": ["shell", "npm", "git"],
        "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
    })
}

/// The endpoint the runs read through: two attributes published, one hidden.
fn mirror(profile: Option<Value>) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "Endpoint",
        "helsinki-bikes",
        PROJECT,
        json!({
            "slug": SLUG,
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "audience": "project",
            "projection": { "hiddenAttributes": ["maintenanceInternalCode"] }
        }),
    ));
    if let Some(spec) = profile {
        mirror.upsert(envelope("AgentProfile", "app-builder", "org", spec));
    }
    mirror
}

fn router(mirror: Arc<Mirror>, config: &Config) -> axum::Router {
    server::app(AppState::new(config.clone(), None).with_mirror(mirror))
}

fn internal_router(mirror: Arc<Mirror>, config: &Config) -> axum::Router {
    server::internal_app(AppState::new(config.clone(), None).with_mirror(mirror))
}

fn with_state(mirror: Arc<Mirror>, config: &Config) -> (AppState, axum::Router, axum::Router) {
    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    (
        state.clone(),
        server::app(state.clone()),
        server::internal_app(state),
    )
}

/// A Portal, its router and its internal router over one shared state, so a run created through
/// the public API is the run the proxy reads.
fn both(mirror: Arc<Mirror>, config: &Config) -> (axum::Router, axum::Router) {
    let (_, app, internal) = with_state(mirror, config);
    (app, internal)
}

fn create_body() -> Value {
    json!({
        "appName": "city-bikes-overview",
        "endpointName": "helsinki-bikes",
        // The workspace shape: a `static` run is the kit pass and never hands out its ticket
        // (tests/kit_pass_tests.rs).
        "appClass": "fullstack",
        "visibility": "project",
        "prompt": "Create a live bike availability dashboard with station filtering",
        "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "types": ["BikeHireDockingStation"],
            "attrs": ["name", "location"],
            "operations": ["queryEntity", "retrieveEntity"]
        }]
    })
}

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", CSRF);
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn internal_call(
    app: &axum::Router,
    bearer: Option<&str>,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Reads the first `frames` frames of an event stream. The stream never ends on its own, so a
/// test that collected the whole body would wait for the keep-alive for ever.
async fn read_stream(
    app: &axum::Router,
    cookie: &str,
    uri: &str,
    last: Option<i64>,
    frames: usize,
) -> String {
    let mut request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header(header::ACCEPT, "text/event-stream");
    if let Some(seq) = last {
        request = request.header("last-event-id", seq.to_string());
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).expect("a request"))
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    assert_eq!(
        response
            .headers()
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no"),
        "the edge would hold every event until the run ended (AG-45)"
    );
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-cache"),
        "the API's own no-store must not replace the stream's header"
    );

    let mut body = response.into_body();
    let mut text = String::new();
    for _ in 0..frames {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("a frame arrives")
            .expect("the stream is open")
            .expect("the frame is data");
        if let Some(chunk) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(chunk));
        }
    }
    text
}

async fn create_run(app: &axum::Router, cookie: &str) -> Value {
    let (status, body) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    body
}

#[tokio::test]
async fn a_run_is_created_queued_and_readable() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let created = create_run(&app, &cookie).await;
    assert_eq!(created["status"], json!("queued"));
    assert_eq!(created["project"], json!(PROJECT));
    assert_eq!(created["endpointSlug"], json!(SLUG));
    assert_eq!(created["allowsWrite"], json!(false));
    assert_eq!(
        created["pathPrefix"],
        json!("projects/helsinki/apps/city-bikes-overview/")
    );
    assert!(
        created["branch"]
            .as_str()
            .is_some_and(|branch| branch.starts_with("agent/app-city-bikes-overview/")),
        "the run owns one branch of the organization repository"
    );
    assert!(
        created["promptDigest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:")),
        "a reviewer has to be able to see the prompt has not been edited"
    );
    assert!(
        created.get("ticketHash").is_none(),
        "AG-46: the ticket hash is never in a public answer"
    );
    assert!(
        created["ticket"].as_str().is_some(),
        "with no cluster the caller drives the workspace, so it needs the ticket"
    );

    let id = created["id"].as_str().expect("an id");
    let (status, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run["id"], json!(id));

    let (status, list) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().map(Vec::len), Some(1));

    let (status, _) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/bb/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another project's run is not found, not forbidden"
    );
}

#[tokio::test]
async fn a_public_application_is_refused_before_anything_is_scheduled() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["visibility"] = json!("public");
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("AP-42")),
        "the refusal names the requirement: {problem}"
    );

    let (status, list) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list["items"].as_array().map(Vec::len),
        Some(0),
        "a refused request leaves no run behind"
    );
}

#[tokio::test]
async fn every_data_need_the_endpoint_hides_is_named_at_once() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["dataNeeds"][0]["attrs"] = json!(["name", "maintenanceInternalCode"]);
    body["dataNeeds"]
        .as_array_mut()
        .expect("needs")
        .push(json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "types": [],
            "attrs": ["maintenanceInternalCode"],
            "operations": ["queryEntity"]
        }));

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let errors = problem["errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        3,
        "AP-44: both hidden attributes and the empty type list, in one answer: {problem}"
    );
}

#[tokio::test]
async fn a_write_operation_marks_the_run_as_writing() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["dataNeeds"][0]["operations"] = json!(["queryEntity", "updateAttrs"]);
    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(
        created["allowsWrite"],
        json!(true),
        "the proxy refuses a write for a run that did not declare one"
    );
}

#[tokio::test]
async fn a_portal_with_no_builder_profile_answers_503() {
    let config = config();
    let app = router(mirror(None), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
}

#[tokio::test]
async fn a_steward_profile_builds_no_application() {
    let config = config();
    let mut spec = builder_profile_spec();
    spec["role"] = json!("steward");
    let app = router(mirror(Some(spec)), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("AG-26")),
        "{problem}"
    );
}

#[tokio::test]
async fn a_portal_with_no_agent_runner_configured_answers_503() {
    let config = Config::for_tests();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
}

#[tokio::test]
async fn the_stream_replays_what_a_reconnecting_browser_missed() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let created = create_run(&app, &cookie).await;
    let id = created["id"].as_str().expect("an id").to_owned();

    // Two more events from the workspace, through the proxy.
    for text in ["reading the model", "scaffolding the view"] {
        let (status, receipt) = internal_call(
            &internal,
            Some(PROXY_TOKEN),
            Method::POST,
            "/internal/agent-runs/events",
            Some(json!({ "runId": id, "kind": "thought", "payload": { "text": text } })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{receipt}");
    }

    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/events");
    let whole = read_stream(&app, &cookie, &uri, None, 3).await;
    assert!(whole.contains("event: status"), "{whole}");
    assert!(whole.contains("id: 1"), "{whole}");
    assert!(whole.contains("scaffolding the view"), "{whole}");
    assert!(
        whole.contains("\"seq\":3"),
        "the sequence number is in the payload as well as on the frame: {whole}"
    );

    let resumed = read_stream(&app, &cookie, &uri, Some(2), 1).await;
    assert!(
        resumed.contains("scaffolding the view") && !resumed.contains("reading the model"),
        "Last-Event-ID replays what came after it and nothing before: {resumed}"
    );
}

#[tokio::test]
async fn an_answer_lands_on_the_runs_log() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/answers"),
        Some(json!({ "questionId": "q-center", "answers": { "center": "Kallio" } })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let stream = read_stream(
        &app,
        &cookie,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/events"),
        Some(1),
        1,
    )
    .await;
    assert!(stream.contains("event: answer"), "{stream}");
    assert!(stream.contains("Kallio"), "{stream}");
    assert!(
        stream.contains(STEWARD),
        "who answered is part of the record: {stream}"
    );
}

#[tokio::test]
async fn cancelling_ends_the_run_and_the_ticket_with_it() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, context) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        &format!("/internal/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        context["ticketHash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("$argon2id$")),
        "the proxy verifies a ticket against a hash, never against a secret: {context}"
    );

    let (status, cancelled) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["status"], json!("cancelled"));
    assert!(cancelled["finishedAt"].as_str().is_some());

    let (_, context) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        &format!("/internal/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(
        context["ticketHash"],
        json!(""),
        "AG-46: an empty hash is not a PHC string, so no ticket verifies against it again"
    );

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a run that is over is over: {problem}"
    );

    let (status, problem) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::POST,
        "/internal/agent-runs/events",
        Some(json!({ "runId": id, "kind": "thought", "payload": { "text": "still here" } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a cancelled run takes no more events: {problem}"
    );
}

#[tokio::test]
async fn a_need_the_app_kind_cannot_parse_is_refused_before_the_run_exists() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    // `contextSpaceRef` is a jc-core `Ref`: a bare name, or `{ kind, name }`. A caller that
    // nests one inside the other builds a need the `App` kind cannot parse, and before this
    // guard the run was accepted and only failed when the person clicked publish.
    let mut body = create_body();
    body["dataNeeds"][0]["contextSpaceRef"]["name"] =
        json!({ "kind": "ContextSpace", "name": "helsinki" });
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    let errors = problem["errors"].as_array().expect("named violations");
    assert!(
        errors
            .iter()
            .filter_map(Value::as_str)
            .any(|error| error.contains("dataNeeds[0]")),
        "the need is named: {problem}"
    );
}

#[tokio::test]
async fn the_published_manifest_is_an_app_the_platform_can_parse() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let created = create_run(&app, &cookie).await;

    // What `publish` writes, from the run as it was stored: a Change is refused without a forge,
    // so the manifest itself is what this asserts, against the kind that has to accept it.
    let manifest = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "App",
        "metadata": { "name": created["appName"], "namespace": PROJECT },
        "spec": {
            "kind": created["appClass"],
            "source": { "path": "./src" },
            "build": { "node": "22" },
            "visibility": created["visibility"],
            "lifecycle": "published",
            "dataNeeds": created["dataNeeds"],
        },
    });
    let spec: jc_core::kinds::AppSpec = serde_json::from_value(manifest["spec"].clone())
        .unwrap_or_else(|error| panic!("the App kind refuses what publish writes: {error}"));
    assert_eq!(spec.data_needs.len(), 1);
}

#[tokio::test]
async fn a_person_steers_a_live_run_and_the_workspace_reads_it() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "  Sort by free bikes, and add a district filter.  " })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The workspace reads one channel: what was answered and what was said, after `after`.
    let (status, body) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        &format!("/internal/agent-runs/{id}/inbox?after=0"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(items[0]["kind"], "message");
    assert_eq!(
        items[0]["payload"]["text"],
        "Sort by free bikes, and add a district filter."
    );
    assert_eq!(items[0]["payload"]["sentBy"], STEWARD);

    // Read past it and the inbox is empty: `after` is how a workspace does not re-read.
    let seq = items[0]["seq"].as_i64().expect("a seq");
    let (status, body) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        &format!("/internal/agent-runs/{id}/inbox?after={seq}&wait=0"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["items"]
            .as_array()
            .is_some_and(|items| items.is_empty()),
        "{body}"
    );
}

#[tokio::test]
async fn the_inbox_is_the_proxy_token_and_nothing_else() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, _) = internal_call(
        &internal,
        Some("not-the-proxy-token"),
        Method::GET,
        &format!("/internal/agent-runs/{id}/inbox?after=0"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_empty_instruction_is_refused_and_a_finished_run_reads_nothing() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "one more thing" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
}

#[tokio::test]
async fn a_run_with_no_preview_publishes_nothing() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/publish"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("AP-46")),
        "{problem}"
    );
}

#[tokio::test]
async fn the_lifecycle_is_driven_by_the_workspace_through_the_proxy() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    for state in [
        "starting",
        "interviewing",
        "building",
        "testing",
        "previewing",
    ] {
        let (status, body) = internal_call(
            &internal,
            Some(PROXY_TOKEN),
            Method::POST,
            "/internal/agent-runs/events",
            Some(json!({ "runId": id, "kind": "status", "payload": { "status": state } })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{state}: {body}");
    }

    let (status, body) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::POST,
        "/internal/agent-runs/events",
        Some(json!({ "runId": id, "kind": "status", "payload": { "status": "published" } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a workspace does not publish its own application: {body}"
    );

    let (_, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(run["status"], json!("previewing"));
    assert!(run["startedAt"].as_str().is_some());
}

#[tokio::test]
async fn usage_and_the_preview_url_are_recorded_from_the_stream() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    for tokens in [4_120, 1_880] {
        internal_call(
            &internal,
            Some(PROXY_TOKEN),
            Method::POST,
            "/internal/agent-runs/events",
            Some(json!({
                "runId": id,
                "kind": "usage",
                "payload": { "tokensThisStep": tokens }
            })),
        )
        .await;
    }
    internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::POST,
        "/internal/agent-runs/events",
        Some(json!({
            "runId": id,
            "kind": "preview",
            "payload": { "previewUrl": "https://portal.hel.fi/apps/city-bikes-overview/" }
        })),
    )
    .await;

    let (_, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(run["tokensUsed"], json!(6_000));
    assert_eq!(run["steps"], json!(2));
    assert_eq!(
        run["previewUrl"],
        json!("https://portal.hel.fi/apps/city-bikes-overview/")
    );
}

#[tokio::test]
async fn the_internal_listener_answers_the_proxy_and_nobody_else() {
    let config = config();
    let internal = internal_router(mirror(Some(builder_profile_spec())), &config);

    for bearer in [None, Some("not-the-proxy-token")] {
        let (status, _) = internal_call(
            &internal,
            bearer,
            Method::GET,
            "/internal/agent-runs/whatever",
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "AG-52: the ticket hash leaves the Portal for the proxy alone"
        );
    }

    let (status, _) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        "/internal/agent-runs/no-such-run",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_internal_routes_are_not_on_the_public_surface() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    // The routed listener answers unknown paths with the single-page application, so what is
    // asserted is that no answer is a run context: the ticket hash is what must not be there.
    for uri in [
        "/internal/agent-runs/some-run",
        "/api/v1/internal/agent-runs/some-run",
        "/api/v1/projects/helsinki/internal/agent-runs/some-run",
    ] {
        let (_, body) = call(&app, &cookie, Method::GET, uri, None).await;
        assert!(
            body.get("ticketHash").is_none(),
            "{uri} answered a run context on the routed listener (AG-52): {body}"
        );
    }
}

#[tokio::test]
async fn a_caller_who_may_not_propose_an_app_starts_no_run() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    // No bootstrap role and no RoleBinding in the mirror: nothing grants proposing an App.
    let cookie = session_cookie(&config, "curious.reader", &[]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
}

#[tokio::test]
async fn a_request_the_app_kind_would_refuse_is_refused_here() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    for (field, value) in [
        ("appName", json!("Not A Label")),
        ("appClass", json!("wasm")),
        ("visibility", json!("everyone")),
        ("prompt", json!("   ")),
    ] {
        let mut body = create_body();
        body[field] = value.clone();
        let (status, problem) = call(
            &app,
            &cookie,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/agent-runs"),
            Some(body),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{field} = {value} was accepted: {problem}"
        );
    }
}

#[tokio::test]
async fn the_diagnostics_door_answers_the_proxy_for_the_runs_own_project_only() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    // AG-57: the door opens for the proxy alone.
    let (status, _) = internal_call(
        &internal,
        None,
        Method::GET,
        &format!("/internal/agent-runs/{id}/diagnostics/pipeline/hsl-bikes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A run that does not exist has no project to look into.
    let (status, _) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        "/internal/agent-runs/no-such-run/diagnostics/pipeline/hsl-bikes",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Only the components the door knows.
    let (status, problem) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        &format!("/internal/agent-runs/{id}/diagnostics/endpoint/hsl-bikes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    // A pipeline the run's project does not have is not found, whatever other projects hold.
    let (status, problem) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        &format!("/internal/agent-runs/{id}/diagnostics/pipeline/hsl-bikes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");

    // A change needs the forge; without one the door says so instead of inventing a state.
    let (status, problem) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        &format!("/internal/agent-runs/{id}/diagnostics/change/chg-0000000a"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
}

#[tokio::test]
async fn a_second_create_for_the_same_app_while_live_is_conflict_and_succeeds_after_cancel() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let created = create_run(&app, &cookie).await;
    let id1 = created["id"].as_str().expect("id").to_owned();

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains(&id1)),
        "the 409 detail names the live run id: {problem}"
    );

    let (status, list) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().map(Vec::len), Some(1));

    let (status, cancelled) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id1}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["status"], json!("cancelled"));

    let (status, created2) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created2}");
    let id2 = created2["id"].as_str().expect("id2");
    assert_ne!(id1, id2);

    let (status, list2) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list2["items"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn list_runs_filters_by_app_query_parameter() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let created1 = create_run(&app, &cookie).await;
    let id1 = created1["id"].as_str().expect("id").to_owned();

    let mut body2 = create_body();
    body2["appName"] = json!("other-app");
    let (status, created2) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body2),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id2 = created2["id"].as_str().expect("id").to_owned();

    let (status, list1) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?app=city-bikes-overview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items1 = list1["items"].as_array().expect("items");
    assert_eq!(items1.len(), 1);
    assert_eq!(items1[0]["id"], id1);
    assert_eq!(items1[0]["appName"], "city-bikes-overview");

    let (status, list2) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?app=other-app"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items2 = list2["items"].as_array().expect("items");
    assert_eq!(items2.len(), 1);
    assert_eq!(items2[0]["id"], id2);
    assert_eq!(items2[0]["appName"], "other-app");

    let (status, list_none) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?app=nonexistent"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items_none = list_none["items"].as_array().expect("items");
    assert_eq!(items_none.len(), 0);
}

#[tokio::test]
async fn expired_runs_are_reaped_and_ticket_invalidated_while_live_runs_remain() {
    let config = config();
    let (state, app, internal) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let live_created = create_run(&app, &cookie).await;
    let live_id = live_created["id"].as_str().expect("id").to_owned();

    let (_ticket, ticket_hash) = mint_ticket();
    let expired_id = mint_run_id();
    let expired_run = AgentRun {
        id: expired_id.clone(),
        project: PROJECT.to_owned(),
        app_name: "expired-app".to_owned(),
        endpoint_name: "helsinki-bikes".to_owned(),
        endpoint_slug: SLUG.to_owned(),
        profile: "app-builder".to_owned(),
        kind: "application".to_owned(),
        unattended: false,
        continues: None,
        app_class: "fullstack".to_owned(),
        visibility: "project".to_owned(),
        prompt: "Expired prompt".to_owned(),
        prompt_digest: digest_prompt("Expired prompt"),
        data_needs: json!([]),
        allows_write: false,
        branch: format!("agent/app-expired-app/{expired_id}"),
        path_prefix: format!("projects/{PROJECT}/apps/expired-app/"),
        status: "building".to_owned(),
        ticket_hash,
        workspace: None,
        merge_request: None,
        preview_url: None,
        first_frame_ms: None,
        first_version_ms: None,
        files: json!({}),
        steps: 5,
        tokens_used: 1000,
        created_by: STEWARD.to_owned(),
        created_at: "2020-01-01T00:00:00Z".to_owned(),
        started_at: Some("2020-01-01T00:01:00Z".to_owned()),
        finished_at: None,
        expires_at: "2020-01-01T00:20:00Z".to_owned(),
        error: None,
    };
    state
        .agents
        .create_run(&expired_run)
        .await
        .expect("create expired run");

    let (status, context) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        &format!("/internal/agent-runs/{expired_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!context["ticketHash"].as_str().unwrap_or("").is_empty());

    let reaped = reaper::reap_expired(&state).await;
    assert_eq!(reaped, 1);

    let (status, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{expired_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run["status"], json!("expired"));
    assert_eq!(run["error"], json!("lease expired unattended"));
    assert!(run["finishedAt"].as_str().is_some());

    let (_, context) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::GET,
        &format!("/internal/agent-runs/{expired_id}"),
        None,
    )
    .await;
    assert_eq!(
        context["ticketHash"],
        json!(""),
        "the ticket hash is cleared after reaping"
    );

    let (status, problem) = internal_call(
        &internal,
        Some(PROXY_TOKEN),
        Method::POST,
        "/internal/agent-runs/events",
        Some(
            json!({ "runId": expired_id, "kind": "thought", "payload": { "text": "still here" } }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an expired run takes no more events: {problem}"
    );

    let (status, live_run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{live_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(live_run["status"], json!("queued"));
    assert!(live_run.get("error").is_none() || live_run["error"].is_null());

    let reaped_again = reaper::reap_expired(&state).await;
    assert_eq!(reaped_again, 0);
}

#[tokio::test]
async fn a_run_that_never_renders_records_neither_first_frame_nor_first_version() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let created = create_run(&app, &cookie).await;
    let id = created["id"].as_str().expect("an id");

    assert!(
        created.get("firstFrameMs").is_none(),
        "firstFrameMs must be omitted when None: {created}"
    );
    assert!(
        created.get("firstVersionMs").is_none(),
        "firstVersionMs must be omitted when None: {created}"
    );

    let (status, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        run.get("firstFrameMs").is_none(),
        "firstFrameMs must be omitted when None: {run}"
    );
    assert!(
        run.get("firstVersionMs").is_none(),
        "firstVersionMs must be omitted when None: {run}"
    );
}

#[tokio::test]
async fn conversation_starts_with_only_a_message_and_has_kind_conversation() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({ "message": "Find datasets about bikes" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["kind"], json!("conversation"));
    assert_eq!(created["appName"], json!(""));
    assert_eq!(created["endpointName"], json!(""));
    assert_eq!(created["endpointSlug"], json!(""));
    assert_eq!(created["appClass"], json!("static"));
    assert_eq!(created["visibility"], json!("private"));
    assert_eq!(created["unattended"], json!(false));
    assert!(created["continues"].is_null());
    assert_eq!(created["prompt"], json!("Find datasets about bikes"));
    assert_eq!(created["status"], json!("queued"));

    let id = created["id"].as_str().expect("id");

    let (status, filtered) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?app=city-bikes-overview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = filtered["items"].as_array().expect("items");
    assert!(!items.iter().any(|item| item["id"] == id));

    let (status, single) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(single["kind"], json!("conversation"));
}

#[tokio::test]
async fn conversation_needs_propose_permission_on_app() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, "curious.reader", &[]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({ "message": "Hello" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
}

#[tokio::test]
async fn continues_validations_reject_invalid_runs() {
    let config = config();
    let (state, app, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    // 1. Missing run id -> 400
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({
            "message": "Continue please",
            "continues": "00000000-0000-0000-0000-000000000000"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    // 2. Application run (kind is not conversation) -> 400
    let app_run = create_run(&app, &cookie).await;
    let app_run_id = app_run["id"].as_str().expect("id");
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({
            "message": "Continue please",
            "continues": app_run_id
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    // 3. Conversation run of another project -> 400
    let other_run_id = mint_run_id();
    let (_, ticket_hash) = mint_ticket();
    let other_project_run = AgentRun {
        id: other_run_id.clone(),
        project: "espoo".to_owned(),
        app_name: "".to_owned(),
        endpoint_name: "".to_owned(),
        endpoint_slug: "".to_owned(),
        profile: "app-builder".to_owned(),
        kind: "conversation".to_owned(),
        unattended: false,
        continues: None,
        app_class: "static".to_owned(),
        visibility: "private".to_owned(),
        prompt: "Earlier conversation".to_owned(),
        prompt_digest: digest_prompt("Earlier conversation"),
        data_needs: json!([]),
        allows_write: false,
        branch: "".to_owned(),
        path_prefix: "".to_owned(),
        status: "interviewing".to_owned(),
        ticket_hash,
        workspace: None,
        merge_request: None,
        preview_url: None,
        first_frame_ms: None,
        first_version_ms: None,
        files: json!({}),
        steps: 0,
        tokens_used: 0,
        created_by: STEWARD.to_owned(),
        created_at: "2026-09-12T08:00:00Z".to_owned(),
        started_at: None,
        finished_at: None,
        expires_at: "2026-09-12T08:30:00Z".to_owned(),
        error: None,
    };
    state
        .agents
        .create_run(&other_project_run)
        .await
        .expect("create run");

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({
            "message": "Continue please",
            "continues": other_run_id
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
}

#[tokio::test]
async fn kind_filter_in_list_runs() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let app_run = create_run(&app, &cookie).await;
    let app_id = app_run["id"].as_str().expect("id");

    let (status, conv_run) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({ "message": "List test conversation" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let conv_id = conv_run["id"].as_str().expect("id");

    let (status, list) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?kind=conversation"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().expect("items");
    assert!(items.iter().any(|item| item["id"] == conv_id));
    assert!(!items.iter().any(|item| item["id"] == app_id));
    assert!(items.iter().all(|item| item["kind"] == "conversation"));

    let (status, list_app) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?kind=application"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items_app = list_app["items"].as_array().expect("items");
    assert!(items_app.iter().any(|item| item["id"] == app_id));
    assert!(!items_app.iter().any(|item| item["id"] == conv_id));
    assert!(items_app.iter().all(|item| item["kind"] == "application"));
}

#[tokio::test]
async fn caller_without_portal_approver_sees_only_own_runs_while_approver_sees_both() {
    let config = config();
    let (state, app, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    let approver_cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let viewer_cookie = session_cookie(&config, "viewer.user", &["viewer"]);

    let approver_run = create_run(&app, &approver_cookie).await;
    let approver_run_id = approver_run["id"].as_str().expect("id");

    let viewer_run_id = mint_run_id();
    let (_, ticket_hash) = mint_ticket();
    let viewer_run = AgentRun {
        id: viewer_run_id.clone(),
        project: PROJECT.to_owned(),
        app_name: "viewer-app".to_owned(),
        endpoint_name: "helsinki-bikes".to_owned(),
        endpoint_slug: SLUG.to_owned(),
        profile: "app-builder".to_owned(),
        kind: "application".to_owned(),
        unattended: false,
        continues: None,
        app_class: "static".to_owned(),
        visibility: "project".to_owned(),
        prompt: "Viewer app".to_owned(),
        prompt_digest: digest_prompt("Viewer app"),
        data_needs: json!([]),
        allows_write: false,
        branch: format!("agent/app-viewer-app/{viewer_run_id}"),
        path_prefix: format!("projects/{PROJECT}/apps/viewer-app/"),
        status: "building".to_owned(),
        ticket_hash,
        workspace: None,
        merge_request: None,
        preview_url: None,
        first_frame_ms: None,
        first_version_ms: None,
        files: json!({}),
        steps: 0,
        tokens_used: 0,
        created_by: "viewer.user".to_owned(),
        created_at: "2026-09-12T09:00:00Z".to_owned(),
        started_at: None,
        finished_at: None,
        expires_at: "2026-09-12T09:30:00Z".to_owned(),
        error: None,
    };
    state
        .agents
        .create_run(&viewer_run)
        .await
        .expect("create viewer run");

    let (status, list) = call(
        &app,
        &approver_cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().expect("items");
    assert!(items.iter().any(|item| item["id"] == approver_run_id));
    assert!(items.iter().any(|item| item["id"] == viewer_run_id));

    let (status, list_mine) = call(
        &app,
        &approver_cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?mine=true"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items_mine = list_mine["items"].as_array().expect("items");
    assert!(items_mine.iter().any(|item| item["id"] == approver_run_id));
    assert!(!items_mine.iter().any(|item| item["id"] == viewer_run_id));

    let (status, list_viewer) = call(
        &app,
        &viewer_cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items_viewer = list_viewer["items"].as_array().expect("items");
    assert!(items_viewer.iter().any(|item| item["id"] == viewer_run_id));
    assert!(!items_viewer
        .iter()
        .any(|item| item["id"] == approver_run_id));
}

#[tokio::test]
async fn create_run_refuses_kind_conversation() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["kind"] = json!("conversation");
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("/assistant/conversations")),
        "{problem}"
    );
}

#[tokio::test]
async fn analysis_run_is_unattended_and_publish_answers_409() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["kind"] = json!("analysis");
    body["unattended"] = json!(false);
    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["kind"], json!("analysis"));
    assert_eq!(created["unattended"], json!(true));

    let id = created["id"].as_str().expect("id");
    let (status, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run["kind"], json!("analysis"));
    assert_eq!(run["unattended"], json!(true));

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/publish"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("an analysis is never published")),
        "{problem}"
    );
}

#[tokio::test]
async fn dashboard_run_is_unattended_and_publish_answers_409() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["kind"] = json!("dashboard");
    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["kind"], json!("dashboard"));
    assert_eq!(created["unattended"], json!(true));

    let id = created["id"].as_str().expect("id");
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/publish"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("publishing a dashboard run is not available yet")),
        "{problem}"
    );
}
