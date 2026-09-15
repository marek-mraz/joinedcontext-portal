//! The kit pass through the real router (T-0571, AP-56…AP-60, AG-53, AG-54, UI-41).
//!
//! A `static` dashboard is driven by the Portal itself: samples through the proxy, one model
//! call, SEARCH/REPLACE blocks applied to `spec.json`, a preview document. An application is
//! code on the App SDK instead; its cases are at the end of this file (T-0680). The proxy is a wiremock
//! that answers what the driver asks; the cases are the first pass, a repair, a block for a
//! path the run may not write, a chat message as a second pass, the preview route's gates and
//! a proxy that does not answer.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::agents::kit;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{header_regex, method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The LinkML source of the `bikes` model, served by the forge (DM-56).
const BIKES_LINKML: &str = "id: https://hel.fi/models/bikes\nname: bikes\nenums:\n  StationStatus:\n    permissible_values:\n      working: {}\n      closed: {}\nclasses:\n  BikeHireDockingStation:\n    slots: [id, name, location, availableBikeNumber, status, stewardNote]\nslots:\n  id: {}\n  name: { range: string, required: true }\n  location: { range: string }\n  availableBikeNumber: { range: integer, minimum_value: 0 }\n  status: { range: StationStatus }\n  stewardNote: { range: string }\n";

const CSRF: &str = "csrf-token-value";
const PROJECT: &str = "helsinki";
const STEWARD: &str = "demo.steward";
const SLUG: &str = "si6epqkx364lprho5uaigutk274r5grb";

const VALID_SPEC: &str = r#"{
  "title": "Helsinki city bikes",
  "sources": [{ "name": "stations", "type": "BikeHireDockingStation", "attrs": ["name", "location", "availableBikeNumber"] }],
  "filters": [{ "kind": "search", "attrs": ["name"] }],
  "views": [
    { "kind": "stats", "items": [{ "label": "Stations", "agg": "count" }] },
    { "kind": "map", "label": "name", "color": "availableBikeNumber" },
    { "kind": "table", "columns": ["name", "availableBikeNumber"] }
  ]
}"#;

fn answer(prose: &str, blocks: &[(&str, &str, &str)]) -> String {
    let mut text = format!("{prose}\n\n```text\n");
    for (path, search, replace) in blocks {
        text.push_str(&format!(
            "{path}\n<<<<<<< SEARCH\n{search}=======\n{replace}\n>>>>>>> REPLACE\n"
        ));
    }
    text.push_str("```\n");
    text
}

fn anthropic(text: &str) -> Value {
    json!({
        "id": "msg_1", "type": "message", "role": "assistant",
        "content": [{ "type": "text", "text": text }],
        "usage": { "input_tokens": 1200, "output_tokens": 300 }
    })
}

fn openai(text: &str) -> Value {
    json!({
        "id": "chatcmpl-1", "object": "chat.completion",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }],
        "usage": { "total_tokens": 1500 }
    })
}

/// An answer the provider cut at the output budget.
fn openai_cut(text: &str) -> Value {
    let mut body = openai(text);
    body["choices"][0]["finish_reason"] = json!("length");
    body
}

fn config(proxy_base: &str) -> Config {
    // Model Tools answers on the proxy's mock too, under its own prefix (SDK-10).
    let model_tools = format!("{proxy_base}/model-tools");
    Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some(proxy_base),
            "JC_PORTAL_MODEL_TOOLS_URL" => Some(model_tools.as_str()),
            "JC_AGENT_PROXY_TOKEN" => Some("the-token-only-jc-agent-proxy-has"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            "JC_PORTAL_PUBLIC_URL" => Some("https://portal.example.com"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete")
}

fn session_cookie(config: &Config, username: &str) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: Some(format!("{username}@hel.fi")),
            name: None,
            roles: vec!["portal-approver".into()],
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

fn mirror(provider: &str) -> Arc<Mirror> {
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
    mirror.upsert(envelope(
        "Organization",
        "hel",
        "org",
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" }),
    ));
    mirror.upsert(envelope(
        "ContextSpace",
        "helsinki",
        PROJECT,
        json!({ "dataModelRef": { "kind": "DataModel", "name": "bikes" } }),
    ));
    mirror.upsert(envelope(
        "DataModel",
        "bikes",
        PROJECT,
        json!({
            "version": "1.0.0",
            "contextSpaceRef": "helsinki",
            "linkml": "./bikes.linkml.yaml",
            "classes": ["BikeHireDockingStation"]
        }),
    ));
    mirror.upsert(envelope(
        "AgentProfile",
        "app-builder",
        "org",
        json!({
            "role": "builder",
            "runtime": {
                "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            },
            "model": { "provider": provider, "name": "claude-sonnet-5", "maxTokensPerRun": 400000, "reasoningEffort": "medium" },
            "limits": { "stepsPerRun": 120, "wallClock": "PT20M", "concurrentRunsPerOrganization": 2, "requestsPerMinute": 60, "maxResponseBytes": 2097152 },
            "egress": { "allowedHosts": [] },
            "tools": ["shell"],
            "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
        }),
    ));
    mirror
}

/// A Portal and its stub proxy. The proxy answers the sample read with five stations and the
/// model route with the canned answers, in order.
async fn portal(provider: &str, answers: &[String]) -> (axum::Router, String, MockServer) {
    let (_, app, cookie, proxy) = portal_state(provider, answers).await;
    (app, cookie, proxy)
}

async fn portal_state(
    provider: &str,
    answers: &[String],
) -> (AppState, axum::Router, String, MockServer) {
    portal_state_with(provider, answers, None).await
}

/// A forge that has no branch and no file for this run yet, and takes the commit; it serves
/// the `bikes` model's LinkML source, the file the field schema is read from (DM-56).
async fn forge() -> MockServer {
    let forge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/org/manifests/contents/projects/helsinki/spaces/helsinki/datamodels/bikes.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-bikes-linkml",
            "content": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, BIKES_LINKML)
        })))
        .with_priority(1)
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/org/manifests"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(
            "^/api/v1/repos/org/manifests/(branches|contents)/",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/org/manifests/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&forge)
        .await;
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/org/manifests/contents/projects/helsinki/apps/city-bikes-overview/src/spec.json",
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "abc123def456" } })),
        )
        .mount(&forge)
        .await;
    forge
}

async fn portal_with(
    provider: &str,
    answers: &[String],
    forge: Option<&MockServer>,
) -> (axum::Router, String, MockServer) {
    let (_, app, cookie, proxy) = portal_state_with(provider, answers, forge).await;
    (app, cookie, proxy)
}

async fn portal_state_with(
    provider: &str,
    answers: &[String],
    forge: Option<&MockServer>,
) -> (AppState, axum::Router, String, MockServer) {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/data/ngsi-ld/v1/entities"))
        .and(query_param("type", "BikeHireDockingStation"))
        .and(query_param("limit", "5"))
        .and(header_regex("authorization", "^Bearer jcr_[0-9a-f-]+\\.[A-Za-z0-9_-]+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "urn:ngsi-ld:BikeHireDockingStation:001", "type": "BikeHireDockingStation", "name": "Kaivopuisto", "availableBikeNumber": 7, "location": { "type": "Point", "coordinates": [24.95, 60.155] } },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:002", "type": "BikeHireDockingStation", "name": "Laivasillankatu", "availableBikeNumber": 2, "location": { "type": "Point", "coordinates": [24.956, 60.161] } },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:003", "type": "BikeHireDockingStation", "name": "Kapteeninpuistikko", "availableBikeNumber": 0, "location": { "type": "Point", "coordinates": [24.945, 60.158] } },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:004", "type": "BikeHireDockingStation", "name": "Viiskulma", "availableBikeNumber": 11, "location": { "type": "Point", "coordinates": [24.941, 60.161] } },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:005", "type": "BikeHireDockingStation", "name": "Sepänkatu", "availableBikeNumber": 4, "location": { "type": "Point", "coordinates": [24.937, 60.159] } }
        ])))
        .mount(&proxy)
        .await;
    // The rows of the preview: every entities read that is not the five-entity sample.
    Mock::given(method("GET"))
        .and(path("/v1/data/ngsi-ld/v1/entities"))
        .and(query_param("options", "keyValues"))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "urn:ngsi-ld:BikeHireDockingStation:001", "type": "BikeHireDockingStation", "name": "Kaivopuisto", "availableBikeNumber": 7 },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:002", "type": "BikeHireDockingStation", "name": "Laivasillankatu", "availableBikeNumber": 2 }
        ])))
        .mount(&proxy)
        .await;
    let route = if provider == "anthropic" {
        "/v1/llm/messages"
    } else {
        "/v1/llm/chat/completions"
    };
    for text in answers {
        let body = if provider == "anthropic" {
            anthropic(text)
        } else {
            openai(text)
        };
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .up_to_n_times(1)
            .mount(&proxy)
            .await;
    }
    let config = config(&proxy.uri());
    let mut state = AppState::new(config.clone(), None).with_mirror(mirror(provider));
    if let Some(forge) = forge {
        let client = GiteaClient::new(forge.uri().parse().expect("url"), "org", "manifests", "t")
            .expect("a forge client");
        state = state.with_gitea(Arc::new(client));
    }
    let app = server::app(state.clone());
    let cookie = session_cookie(&config, STEWARD);
    (state, app, cookie, proxy)
}

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
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
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (status, headers, bytes.to_vec())
}

async fn json(
    app: &axum::Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, bytes) = call(app, cookie, method, uri, body).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn create_run(app: &axum::Router, cookie: &str) -> Value {
    create_run_with(
        app,
        cookie,
        "Create a live bike availability dashboard with station filtering",
        &["queryEntity"],
    )
    .await
}

async fn create_run_with(
    app: &axum::Router,
    cookie: &str,
    prompt: &str,
    operations: &[&str],
) -> Value {
    let (status, body) = json(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointName": "helsinki-bikes",
            "appClass": "static",
            "kind": "dashboard",
            "visibility": "project",
            "prompt": prompt,
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "location", "availableBikeNumber"],
                "operations": operations
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    body
}

/// The run as read back once its status is one of `statuses`, or a panic after ten seconds
/// with the run as it stands.
async fn wait_for(app: &axum::Router, cookie: &str, id: &str, statuses: &[&str]) -> Value {
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}");
    let mut last = Value::Null;
    for _ in 0..200 {
        let (status, run) = json(app, cookie, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{run}");
        if statuses.iter().any(|s| run["status"] == json!(s)) {
            return run;
        }
        last = run;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the run never reached {statuses:?}: {last}");
}

/// Every recorded event of a run, in order, read from the store rather than the live stream:
/// what a browser that reconnects after the pass is shown.
async fn events(app: &axum::Router, cookie: &str, id: &str) -> Vec<(String, Value)> {
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/events");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(&uri)
                .header(header::COOKIE, cookie)
                .header(header::ACCEPT, "text/event-stream")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let mut text = String::new();
    // The replay is one frame per event; the stream then waits for the next live one, which
    // is the timeout below.
    while let Ok(Some(Ok(frame))) =
        tokio::time::timeout(Duration::from_millis(300), body.frame()).await
    {
        if let Some(chunk) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(chunk));
        }
    }
    text.split("\n\n")
        .filter_map(|frame| {
            let kind = frame
                .lines()
                .find_map(|line| line.strip_prefix("event: "))?;
            let data = frame.lines().find_map(|line| line.strip_prefix("data: "))?;
            Some((kind.to_owned(), serde_json::from_str(data).ok()?))
        })
        .collect()
}

fn kinds(events: &[(String, Value)]) -> Vec<&str> {
    events.iter().map(|(kind, _)| kind.as_str()).collect()
}

async fn model_requests(proxy: &MockServer) -> Vec<Value> {
    proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path().starts_with("/v1/llm/"))
        .map(|request| serde_json::from_slice(&request.body).unwrap_or(Value::Null))
        .collect()
}

#[tokio::test]
async fn the_first_pass_writes_the_spec_and_the_preview_is_one_document() {
    let (app, cookie, proxy) = portal(
        "anthropic",
        &[answer(
            "A dashboard of the city's bike stations: a count, a map coloured by free bikes, a table.",
            &[("spec.json", "", VALID_SPEC)],
        )],
    )
    .await;

    let created = create_run(&app, &cookie).await;
    assert_eq!(created["status"], json!("queued"));
    assert!(
        created["ticket"].is_null(),
        "a kit run keeps its ticket (AG-54): {created}"
    );
    let id = created["id"].as_str().expect("an id").to_owned();

    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");
    // The clock AP-57 is measured on: set by the time the first preview is there, and by the
    // Portal, not the model.
    let first_frame = run["firstFrameMs"]
        .as_i64()
        .expect("firstFrameMs on a previewing run");
    assert!((0..60_000).contains(&first_frame), "{first_frame}");
    // The first version is the first pass's applied specification: never before the frame (AG-66).
    let first_version = run["firstVersionMs"]
        .as_i64()
        .expect("firstVersionMs once the first pass is served");
    assert!(
        (first_frame..60_000).contains(&first_version),
        "{first_frame} {first_version}"
    );
    let preview_url = run["previewUrl"]
        .as_str()
        .expect("a preview url")
        .to_owned();
    assert!(
        preview_url.starts_with(&format!(
            "/api/v1/projects/{PROJECT}/agent-runs/{id}/preview?v="
        )),
        "{preview_url}"
    );

    // The model was asked once, through the proxy, with the samples and the prompt in the body
    // and the run's ticket as the bearer (AP-57, AG-53).
    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 1);
    // The rows were read once the specification stood, with its attributes and its limit.
    let reads: Vec<String> = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/v1/data/ngsi-ld/v1/entities")
        .map(|r| r.url.query().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(reads.len(), 2, "{reads:?}");
    assert!(reads[0].contains("limit=5"), "{reads:?}");
    assert!(
        reads[1].contains("limit=500")
            && reads[1].contains("attrs=name%2Clocation%2CavailableBikeNumber"),
        "{reads:?}"
    );
    let body = requests[0].to_string();
    assert!(body.contains("Kaivopuisto"), "the samples are in the pack");
    assert!(
        body.contains("station filtering"),
        "the prompt is in the pack"
    );
    assert!(
        body.contains("\"system\""),
        "the anthropic body carries a system field"
    );
    assert!(body.contains("ONE-SHOT DASHBOARD SPECIFICATION"));
    assert!(
        !body.contains("the-token-only-jc-agent-proxy-has"),
        "the proxy token never leaves the Portal"
    );

    let log = events(&app, &cookie, &id).await;
    let kinds = kinds(&log);
    for kind in ["status", "thought", "tool", "preview"] {
        assert!(kinds.contains(&kind), "{kinds:?} lacks {kind}");
    }
    let statuses: Vec<&str> = log
        .iter()
        .filter(|(kind, _)| kind == "status")
        .filter_map(|(_, payload)| payload["status"].as_str())
        .collect();
    assert_eq!(
        statuses,
        [
            "queued",
            "starting",
            "building",
            "testing",
            "previewing",
            "awaiting_approval"
        ]
    );
    let tool = log
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == json!("apply_patch"))
        .expect("the apply_patch tool event");
    assert_eq!(tool.1["exitCode"], json!(0));
    assert_eq!(tool.1["applied"][0]["path"], json!("spec.json"));
    let preview = log
        .iter()
        .find(|(kind, _)| kind == "preview")
        .expect("a preview event");
    assert_eq!(preview.1["previewUrl"], json!(preview_url));
    assert!(log.iter().any(|(kind, payload)| kind == "thought"
        && payload["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("A dashboard of the city"))));
    // The chat never names the model: that is the profile's business, not the person's.
    assert!(!log.iter().any(|(_, payload)| payload["text"]
        .as_str()
        .is_some_and(|t| t.contains("claude-sonnet-5"))));

    // The preview: one document with its own policy, or 503 in a build without the bundle.
    let (status, headers, bytes) = call(&app, &cookie, Method::GET, &preview_url, None).await;
    match kit::bundle() {
        Some(bundle) => {
            assert_eq!(status, StatusCode::OK);
            let html = String::from_utf8_lossy(&bytes);
            assert!(html.contains("<script id=\"kit-spec\" type=\"application/json\">"));
            assert!(html.contains(&format!("\"slug\":\"{SLUG}\"")));
            assert!(html.contains("Helsinki city bikes"));
            // serde_json orders keys, so the row is matched by two of them rather than by shape.
            assert!(
                html.contains("\"data\":{\"stations\":[{"),
                "the rows travel with the document"
            );
            assert!(html.contains("\"name\":\"Laivasillankatu\""));
            let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
            assert!(csp.contains(&kit::script_hash(&bundle.js)), "{csp}");
            assert!(html.contains("<script id=\"kit-worker\" type=\"text/plain\">"));
            assert!(
                csp.contains(
                    "connect-src https://portal.example.com https://tiles.openfreemap.org"
                ),
                "{csp}"
            );
            assert!(
                !csp.contains("script-src 'self'"),
                "the Portal's own policy must not replace the document's"
            );
            assert_eq!(headers[header::CACHE_CONTROL], "no-store");
            assert_eq!(headers["x-frame-options"], "SAMEORIGIN");
        }
        None => assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            String::from_utf8_lossy(&bytes)
        ),
    }

    // Without a session the document is not served: it is the run's data, not a public page.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&preview_url)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_answer_that_does_not_validate_is_repaired_once() {
    let broken = VALID_SPEC.replace("\"availableBikeNumber\" }", "\"nope\" }");
    let (app, cookie, proxy) = portal(
        "openai-compatible",
        &[
            answer("First try.", &[("spec.json", "", &broken)]),
            answer(
                "Fixed the colour attribute.",
                &[("spec.json", "", VALID_SPEC)],
            ),
        ],
    )
    .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");

    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 2, "one pass, one repair");
    let repair = requests[1].to_string();
    assert!(
        repair.contains("views[1].color: 'nope' is not among the source's attributes"),
        "{repair}"
    );
    assert!(
        repair.contains("\"role\":\"system\""),
        "the openai body carries a system message"
    );
    let log = events(&app, &cookie, &id).await;
    assert!(log.iter().any(|(kind, payload)| kind == "thought"
        && payload["text"]
            .as_str()
            .is_some_and(|t| t.contains("asking for a repair"))));
    assert_eq!(log.iter().filter(|(kind, _)| kind == "preview").count(), 1);
}

#[tokio::test]
async fn a_block_for_a_foreign_path_is_refused_and_the_spec_still_lands() {
    let (app, cookie, _proxy) = portal(
        "anthropic",
        &[answer(
            "Done.",
            &[
                ("kit/src/App.tsx", "", "export default 1;"),
                ("../../etc/passwd", "", "x"),
                ("spec.json", "", VALID_SPEC),
            ],
        )],
    )
    .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    // Refused blocks are not a validation problem: the specification is there and valid, so
    // no repair is asked for and the preview is shown (AP-58).
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");
    let log = events(&app, &cookie, &id).await;
    let tool = log
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == json!("apply_patch"))
        .expect("the apply_patch tool event");
    assert_eq!(
        tool.1["refused"].as_array().map(Vec::len),
        Some(2),
        "{}",
        tool.1
    );
    assert_eq!(tool.1["applied"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn a_message_is_one_more_pass_over_the_same_file() {
    let (app, cookie, proxy) = portal(
        "anthropic",
        &[
            answer("Built.", &[("spec.json", "", VALID_SPEC)]),
            answer(
                "Renamed the dashboard.",
                &[(
                    "spec.json",
                    "{\n  \"title\": \"Helsinki city bikes\",\n  \"sources\": [{ \"name\": \"stations\", \"type\": \"BikeHireDockingStation\", \"attrs\": [\"name\", \"location\", \"availableBikeNumber\"] }],\n",
                    "{\n  \"title\": \"Kaupunkipyörät\",\n  \"sources\": [{ \"name\": \"stations\", \"type\": \"BikeHireDockingStation\", \"attrs\": [\"name\", \"location\", \"availableBikeNumber\"] }],",
                )],
            ),
        ],
    )
    .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");
    let first = run["previewUrl"].as_str().unwrap().to_owned();

    let (status, _) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "Call it Kaupunkipyörät" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let mut run = Value::Null;
    for _ in 0..200 {
        run = json(
            &app,
            &cookie,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
            None,
        )
        .await
        .1;
        if run["previewUrl"] != json!(first) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_ne!(
        run["previewUrl"],
        json!(first),
        "a second pass moves the preview: {run}"
    );
    assert_eq!(run["status"], json!("awaiting_approval"));

    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 2);
    let second = requests[1].to_string();
    assert!(second.contains("Call it Kaupunkipyörät"), "{second}");
    assert!(
        second.contains("### spec.json"),
        "the current file is in the pack: {second}"
    );
    assert!(
        !second.contains("### data.json"),
        "the rows stay out of the prompt: {second}"
    );
    assert!(
        second.contains("Person: Create a live bike"),
        "the conversation is in the pack"
    );
    if kit::bundle().is_some() {
        let (status, _, bytes) = call(
            &app,
            &cookie,
            Method::GET,
            run["previewUrl"].as_str().unwrap(),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(String::from_utf8_lossy(&bytes).contains("Kaupunkipyörät"));
    }
}

#[tokio::test]
async fn an_empty_answer_is_asked_again_once() {
    let (app, cookie, proxy) = portal(
        "anthropic",
        &[
            "".to_owned(),
            answer("Second try.", &[("spec.json", "", VALID_SPEC)]),
        ],
    )
    .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");
    assert_eq!(model_requests(&proxy).await.len(), 2);
}

#[tokio::test]
async fn the_preview_is_gated_on_the_run_and_its_project() {
    let (app, cookie, _proxy) = portal("anthropic", &[]).await;
    let (status, body) = json(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/no-such-run/preview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A run whose pass has not happened yet has no document (API/04 "Preview Document").
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = json(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/tampere/agent-runs/{id}/preview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_proxy_that_does_not_answer_fails_the_run_with_the_reason() {
    // No canned answer mounted: the model route answers 404, which the driver reports.
    let (app, cookie, _proxy) = portal("anthropic", &[]).await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["failed", "awaiting_approval"]).await;
    assert_eq!(run["status"], json!("failed"), "{run}");
    assert!(
        run["error"]
            .as_str()
            .is_some_and(|e| e.contains("the proxy answered 404")),
        "{run}"
    );
    assert!(run["previewUrl"].is_null());
}

#[tokio::test]
async fn a_key_short_of_credit_is_asked_again_within_what_it_covers() {
    let (app, cookie, proxy) = portal("anthropic", &[]).await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/messages"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({ "error": {
            "code": 402,
            "message": "You requested up to 24000 tokens, but can only afford 20000. To increase, visit https://openrouter.ai/workspaces/default/keys/abc"
        } })))
        .up_to_n_times(1)
        .mount(&proxy)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(anthropic(&answer(
            "Bikes.",
            &[("spec.json", "", VALID_SPEC)],
        ))))
        .up_to_n_times(1)
        .mount(&proxy)
        .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");
    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["max_tokens"], json!(19000));
}

#[tokio::test]
async fn a_key_out_of_credit_fails_the_pass_without_the_providers_links() {
    let (app, cookie, proxy) = portal("anthropic", &[]).await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/messages"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({ "error": {
            "code": 402,
            "message": "You requested up to 24000 tokens, but can only afford 1200. To increase, visit https://openrouter.ai/workspaces/default/keys/abc"
        } })))
        .mount(&proxy)
        .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["failed", "awaiting_approval"]).await;
    assert_eq!(run["status"], json!("failed"), "{run}");
    let error = run["error"].as_str().unwrap_or_default();
    assert!(error.contains("credit") && error.contains("1200"), "{run}");
    assert!(!error.contains("openrouter.ai"), "{run}");
    assert_eq!(model_requests(&proxy).await.len(), 1);
}

#[tokio::test]
async fn every_pass_is_a_commit_on_the_run_branch_when_there_is_a_forge() {
    let forge = forge().await;
    let (app, cookie, _proxy) = portal_with(
        "anthropic",
        &[answer("Bikes.", &[("spec.json", "", VALID_SPEC)])],
        Some(&forge),
    )
    .await;
    let created = create_run(&app, &cookie).await;
    let id = created["id"].as_str().expect("id");
    wait_for(&app, &cookie, id, &["awaiting_approval", "failed"]).await;

    let events = events(&app, &cookie, id).await;
    let commit = events
        .iter()
        .find(|(kind, _)| kind == "commit")
        .unwrap_or_else(|| panic!("no commit among {:?}", kinds(&events)));
    assert_eq!(commit.1["sha"], json!("abc123def456"));
    let put = forge
        .received_requests()
        .await
        .expect("recording")
        .into_iter()
        .find(|r| r.method == "PUT")
        .expect("the file was written");
    let body: Value = serde_json::from_slice(&put.body).expect("json");
    assert!(
        body["branch"]
            .as_str()
            .is_some_and(|b| b.starts_with("agent/app-city-bikes-overview/")),
        "{body}"
    );
    assert_eq!(body["message"], json!("Bikes."));
}

#[tokio::test]
async fn an_answer_that_changes_no_file_is_a_reply_not_a_version() {
    let forge = forge().await;
    let refusal = "A data model is made in the data-model editor, not in this dashboard.";
    let (app, cookie, proxy) = portal_with(
        "anthropic",
        &[
            answer("Bikes.", &[("spec.json", "", VALID_SPEC)]),
            answer(refusal, &[]),
        ],
        Some(&forge),
    )
    .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");
    let (status, _) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "create a new data model" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let mut seen = Vec::new();
    for _ in 0..100 {
        seen = events(&app, &cookie, &id).await;
        if seen.iter().any(|(_, p)| p["text"] == json!(refusal)) {
            break;
        }
    }
    let count = |kind: &str| seen.iter().filter(|(k, _)| k == kind).count();
    assert!(
        seen.iter().any(|(_, p)| p["text"] == json!(refusal)),
        "the reply reaches the chat: {:?}",
        kinds(&seen)
    );
    assert_eq!(count("commit"), 1, "{:?}", kinds(&seen));
    assert_eq!(count("preview"), 1, "{:?}", kinds(&seen));
    assert_eq!(model_requests(&proxy).await.len(), 2);
    let run = json(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await
    .1;
    assert_eq!(
        run["previewUrl"],
        json!(format!(
            "/api/v1/projects/{PROJECT}/agent-runs/{id}/preview?v=1"
        ))
    );
}

#[tokio::test]
async fn a_page_beside_the_spec_replaces_the_kit_in_the_preview() {
    const PAGE: &str = "<!doctype html><html><head><title>3D</title>\
        <script src=\"https://cdn.jsdelivr.net/npm/three@0.160.0/build/three.min.js\"></script>\
        </head><body><canvas id=\"scene\"></canvas><script>const rows = window.kit.data.stations;</script></body></html>";
    let (app, cookie, _proxy) = portal(
        "anthropic",
        &[answer(
            "A 3D scene of the stations, one column per station.",
            &[("spec.json", "", VALID_SPEC), ("index.html", "", PAGE)],
        )],
    )
    .await;
    let created = create_run(&app, &cookie).await;
    let id = created["id"].as_str().expect("id");
    let run = wait_for(&app, &cookie, id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");

    let (status, headers, body) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let csp = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        csp.contains("script-src 'unsafe-inline' https://cdn.jsdelivr.net"),
        "{csp}"
    );
    assert!(!csp.contains("sha256"), "the page is not the kit: {csp}");
    let body = String::from_utf8(body).expect("utf8");
    assert!(
        body.contains("<script>window.kit = "),
        "the rows are inlined"
    );
    assert!(
        body.contains("Laivasillankatu"),
        "the rows the driver read are in the page"
    );
    assert!(
        body.contains("three.min.js"),
        "the page is served as written"
    );
    assert!(!body.contains("kit-worker"), "the kit itself is not loaded");
}

#[tokio::test]
async fn a_cut_answer_is_refused_whole_and_the_chat_says_so() {
    // The first pass is fine; the message's pass comes back cut, and the preview stays.
    let (app, cookie, proxy) = portal_with("openai-compatible", &[], None).await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(openai(&answer("Bikes.", &[("spec.json", "", VALID_SPEC)]))),
        )
        .up_to_n_times(1)
        .mount(&proxy)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_cut(
            "A 3D scene.\n\n```text\nindex.html\n<<<<<<< SEARCH\n=======\n<!doctype html><html><body><script>const half",
        )))
        .mount(&proxy)
        .await;
    let created = create_run(&app, &cookie).await;
    let id = created["id"].as_str().expect("id");
    wait_for(&app, &cookie, id, &["awaiting_approval", "failed"]).await;
    let (status, _, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "Make it 3D." })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "the message is taken");
    let mut said = false;
    for _ in 0..50 {
        let events = events(&app, &cookie, id).await;
        said = events.iter().any(|(kind, payload)| {
            kind == "thought"
                && payload["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("cut at the output budget"))
        });
        if said {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(said, "the chat names the cut");
    let run = wait_for(&app, &cookie, id, &["awaiting_approval"]).await;
    assert_eq!(
        run["previewUrl"],
        json!(format!(
            "/api/v1/projects/{PROJECT}/agent-runs/{id}/preview?v=1"
        )),
        "the first preview stands"
    );
}

#[tokio::test]
async fn a_share_request_is_a_proposal_and_a_navigation_not_a_pass() {
    let (app, cookie, proxy) = portal(
        "anthropic",
        &[
            answer("Built.", &[("spec.json", "", VALID_SPEC)]),
            concat!(
                "Drafted the endpoint for the regional transport team, with the maintenance notes hidden.\n\n",
                "```json\n",
                "{\"tool\": \"propose_endpoint\", \"contextSpace\": \"helsinki\", \"name\": \"bikes-regional-transport\", ",
                "\"title\": \"City bikes for the regional transport team\", \"allowedProjects\": [\"regional-transport\"], ",
                "\"hiddenAttributes\": [\"maintenanceNote\"], \"entityTypes\": [\"BikeHireDockingStation\"]}\n",
                "```\n"
            )
            .to_owned(),
        ],
    )
    .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");
    let first = run["previewUrl"].as_str().unwrap().to_owned();

    let (status, _) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "Share the bike stations with the regional transport team, but hide the maintenance notes" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let mut all = Vec::new();
    for _ in 0..200 {
        all = events(&app, &cookie, &id).await;
        if all.iter().any(|(kind, _)| kind == "navigate") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let tool = all
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == json!("propose_endpoint"))
        .expect("the proposal is a tool step (AG-56)");
    assert_eq!(tool.1["status"], json!("ok"), "{}", tool.1);
    assert_eq!(tool.1["output"]["lane"], json!("yellow"));
    assert_eq!(tool.1["output"]["slug"].as_str().map(str::len), Some(26));
    assert_eq!(
        tool.1["output"]["endpoint"]["spec"]["projection"]["hiddenAttributes"],
        json!(["maintenanceNote"])
    );
    assert_eq!(
        tool.1["output"]["policies"][0]["spec"]["assignee"],
        json!({ "kind": "group", "id": "regional-transport" })
    );
    let navigate = all
        .iter()
        .find(|(kind, _)| kind == "navigate")
        .expect("the form is opened for the person (UI-45)");
    assert_eq!(
        navigate.1["route"],
        json!(format!("/projects/{PROJECT}/endpoints"))
    );
    assert_eq!(
        navigate.1["prefill"]["name"],
        json!("bikes-regional-transport")
    );
    assert_eq!(navigate.1["prefill"]["audience"], json!("project-list"));
    assert_eq!(
        navigate.1["prefill"]["hiddenAttributes"],
        json!(["maintenanceNote"])
    );
    assert_eq!(navigate.1["prefill"]["slug"], tool.1["output"]["slug"]);
    assert!(
        all.iter().any(|(kind, payload)| kind == "thought"
            && payload["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("Drafted the endpoint for the regional"))),
        "the sentences before the block are the chat line"
    );

    let (_, run) = json(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(
        run["previewUrl"],
        json!(first),
        "the dashboard did not move: {run}"
    );
    assert_eq!(run["status"], json!("awaiting_approval"));
    assert_eq!(
        model_requests(&proxy).await.len(),
        2,
        "no repair call for a tool answer"
    );
}

const EDIT_SPEC: &str = r#"{
  "title": "Station notes",
  "sources": [{ "name": "stations", "type": "BikeHireDockingStation", "attrs": ["name", "location", "availableBikeNumber"] }],
  "views": [
    { "kind": "table", "columns": ["name", "availableBikeNumber"] },
    { "kind": "form", "title": "Station", "fields": ["availableBikeNumber"] }
  ]
}"#;

/// T-0595 (AP-61, AP-62): an application that may write gets a table and a form, the pack
/// carries the field schema of the space's model, and the preview inlines the same schema with
/// the bridge flag; an application that may not write is told to drop the form.
#[tokio::test]
async fn an_edit_prompt_gets_a_form_grounded_in_the_field_schema_when_the_app_may_write() {
    let forge = forge().await;
    let (app, cookie, proxy) = portal_with(
        "openai-compatible",
        &[answer(
            "A table of the stations and a form to update a station's free bikes.",
            &[("spec.json", "", EDIT_SPEC)],
        )],
        Some(&forge),
    )
    .await;
    let id = create_run_with(
        &app,
        &cookie,
        "Build an app where stewards can update bike station notes",
        &["queryEntity", "updateAttrs"],
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");

    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 1);
    let body = requests[0].to_string();
    assert!(
        body.contains("WHEN THE PERSON ASKS TO EDIT, UPDATE OR MANAGE ENTITIES"),
        "the system prompt pairs a table and a form"
    );
    assert!(body.contains("This application MAY write"), "{body}");
    assert!(
        body.contains(r#"\"availableBikeNumber\""#) && body.contains(r#"\"minimum\": 0"#),
        "the field schema of the model is packed: {body}"
    );
    assert!(
        body.contains(r#"\"enum\""#) && body.contains("closed"),
        "{body}"
    );

    let preview_url = run["previewUrl"]
        .as_str()
        .expect("a preview url")
        .to_owned();
    let (status, _, bytes) = call(&app, &cookie, Method::GET, &preview_url, None).await;
    assert_eq!(status, StatusCode::OK);
    let html = String::from_utf8(bytes).expect("utf-8");
    assert!(
        html.contains("\"bridge\":true"),
        "a preview writes through the host page (AP-63)"
    );
    assert!(
        html.contains("\"schema\":{\"BikeHireDockingStation\":{\"properties\""),
        "the form's inputs come from the same schema (AP-61)"
    );
    // The kit names the cookie it reads (`jc_csrf`); the session's values never appear.
    assert!(
        !html.contains(CSRF) && !html.contains("jcr_"),
        "no credential is inlined"
    );
}

#[tokio::test]
async fn a_form_in_an_app_that_may_not_write_is_sent_back_for_repair() {
    let (app, cookie, proxy) = portal(
        "openai-compatible",
        &[
            answer("With a form.", &[("spec.json", "", EDIT_SPEC)]),
            answer("Without the form.", &[("spec.json", "", VALID_SPEC)]),
        ],
    )
    .await;
    let id = create_run_with(
        &app,
        &cookie,
        "Build an app where stewards can update bike station notes",
        &["queryEntity"],
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");
    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 2, "one pass, one repair");
    assert!(requests[0]
        .to_string()
        .contains("This application may NOT write"));
    assert!(
        requests[1].to_string().contains(
            "views[1]: a form writes through the endpoint and this application may not write"
        ),
        "{}",
        requests[1]
    );
}

/// T-0583 (PF-54, PF-55, UI-17): an indicator asked for in the chat is computed from what the
/// endpoint serves, rendered with its provenance, handed over as a `tool` step for the card,
/// and written by nobody: the run's files and the preview stay as they were.
#[tokio::test]
async fn a_kpi_request_is_computed_from_the_endpoint_and_handed_to_the_person() {
    let call = "Average free bikes across the stations, as an indicator.\n\n```json\n{\"tool\":\"compute_kpi\",\"name\":\"average-free-bikes\",\"title\":\"Average free bikes\",\"type\":\"BikeHireDockingStation\",\"attribute\":\"availableBikeNumber\",\"agg\":\"avg\",\"unit\":\"C62\"}\n```";
    let (app, cookie, proxy) = portal(
        "anthropic",
        &[
            answer("A dashboard.", &[("spec.json", "", VALID_SPEC)]),
            call.to_owned(),
        ],
    )
    .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");
    let before = events(&app, &cookie, &id).await.len();

    let (status, _) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "What is the average number of free bikes? Make it a KPI." })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let log = loop {
        let log = events(&app, &cookie, &id).await;
        if log.len() > before
            && log
                .iter()
                .any(|(kind, payload)| kind == "tool" && payload["tool"] == "compute_kpi")
        {
            break log;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let (_, step) = log
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == "compute_kpi")
        .expect("the kpi step");
    assert_eq!(step["status"], "ok", "{step}");
    let output = &step["output"];
    // The preview rows the proxy serves: 7 and 2 free bikes.
    assert_eq!(output["value"], 4.5);
    assert_eq!(output["count"], 2);
    assert_eq!(output["space"], "helsinki-kpi");
    assert_eq!(
        output["formula"],
        "avg(availableBikeNumber) over BikeHireDockingStation"
    );
    assert!(
        output["endpointSlug"].is_null(),
        "no indicator endpoint in this mirror"
    );
    let entity = &output["entity"];
    assert_eq!(
        entity["id"],
        "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:average-free-bikes"
    );
    assert_eq!(
        entity["derivedFrom"]["object"],
        "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-bikes"
    );
    assert_eq!(
        entity["computedBy"]["object"],
        format!("urn:ngsi-ld:AgentRun:hel.fi:helsinki:{id}")
    );
    assert_eq!(entity["currentValue"]["unitCode"], "C62");
    // Nothing was written anywhere: no POST reached the proxy's data route, and the
    // dashboard's preview count did not move.
    let writes = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method == "POST" && r.url.path().starts_with("/v1/data/"))
        .count();
    assert_eq!(writes, 0, "the assistant writes no entity (AG-20)");
    assert_eq!(log.iter().filter(|(kind, _)| kind == "preview").count(), 1);
    assert!(log.iter().any(|(kind, payload)| kind == "thought"
        && payload["text"]
            .as_str()
            .is_some_and(|t| t.contains("Average free bikes"))));
}

#[tokio::test]
async fn a_conversation_answers_prose_stays_interviewing_and_answers_second_message() {
    let (app, cookie, _proxy) = portal(
        "anthropic",
        &[
            "You can find bike stations using the catalog search or explore view.".to_owned(),
            "Station Kaivopuisto currently has 7 available bikes.".to_owned(),
        ],
    )
    .await;

    let (status, created) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({
            "message": "Where are the bike stations?"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["kind"], json!("conversation"));
    assert_eq!(created["status"], json!("queued"));
    let id = created["id"].as_str().expect("an id").to_owned();

    let run = wait_for(&app, &cookie, &id, &["interviewing", "failed"]).await;
    assert_eq!(run["status"], json!("interviewing"), "{run}");
    assert!(
        run["previewUrl"].is_null(),
        "conversation runs have no preview"
    );

    let log = events(&app, &cookie, &id).await;
    assert!(
        log.iter().any(|(kind, payload)| kind == "thought"
            && payload["text"]
                .as_str()
                .is_some_and(|t| t.contains("You can find bike stations"))),
        "the model answer was published as a thought: {log:?}"
    );

    let (status, _) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "How many bikes at Kaivopuisto?" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let mut said_second = false;
    for _ in 0..100 {
        let log = events(&app, &cookie, &id).await;
        said_second = log.iter().any(|(kind, payload)| {
            kind == "thought"
                && payload["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("Station Kaivopuisto currently has 7"))
        });
        if said_second {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(said_second, "second message received second thought answer");

    let run_after = wait_for(&app, &cookie, &id, &["interviewing"]).await;
    assert_eq!(run_after["status"], json!("interviewing"));
    assert!(run_after["previewUrl"].is_null());
}

#[tokio::test]
async fn an_unattended_analysis_run_ends_awaiting_approval_with_report_md() {
    const REPORT_MD: &str =
        "# Station Bike Analysis\n\nOverall bike availability across stations is 4.8.";
    let (state, app, cookie, _proxy) = portal_state(
        "anthropic",
        &[answer(
            "Analysis complete with visual dashboard and report.",
            &[("spec.json", "", VALID_SPEC), ("report.md", "", REPORT_MD)],
        )],
    )
    .await;

    let (status, body) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-analysis",
            "endpointName": "helsinki-bikes",
            "appClass": "static",
            "visibility": "project",
            "kind": "analysis",
            "unattended": true,
            "prompt": "Analyze station bike availability",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "location", "availableBikeNumber"],
                "operations": ["queryEntity"]
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["kind"], json!("analysis"));
    assert_eq!(body["unattended"], json!(true));
    let id = body["id"].as_str().expect("an id").to_owned();

    let run = wait_for(&app, &cookie, &id, &["awaiting_approval", "failed"]).await;
    assert_eq!(run["status"], json!("awaiting_approval"), "{run}");

    let stored = state
        .agents
        .get_run(&id)
        .await
        .expect("get_run")
        .expect("run exists in store");
    assert_eq!(stored.status, "awaiting_approval");
    assert!(
        stored.files.get("spec.json").is_some(),
        "spec.json must be in files"
    );
    assert_eq!(
        stored
            .files
            .get("report.md")
            .and_then(Value::as_str)
            .map(str::trim_end),
        Some(REPORT_MD),
        "report.md must be in files"
    );
}

// ---- An application: code on the App SDK (T-0680, SDK-10…SDK-17) ----

/// The row types Model Tools renders for the `bikes` model in these cases.
const JC_TYPES: &str = "export interface BikeHireDockingStation { id: string; type: \"BikeHireDockingStation\"; name?: string; availableBikeNumber?: number }\n";

/// A page and its test that build: the SDK hook, the row type as a type-only import.
const STATIONS: &str = "import { useEntities } from \"@joinedcontext/sdk\";\nimport type { BikeHireDockingStation } from \"../jc-types\";\n\nexport function Stations() {\n  const { rows } = useEntities<BikeHireDockingStation>(\"BikeHireDockingStation\");\n  return <ul>{rows.map((row) => <li key={row.id}>{row.name}</li>)}</ul>;\n}";
const STATIONS_TEST: &str = "import { render } from \"@testing-library/react\";\nimport { describe, expect, it } from \"vitest\";\nimport { JcProvider } from \"@joinedcontext/sdk\";\nimport { stubClient } from \"@joinedcontext/sdk/testing\";\nimport { Stations } from \"./Stations\";\n\ndescribe(\"Stations\", () => {\n  it(\"lists the stations\", () => {\n    const { container } = render(<JcProvider client={stubClient()}><Stations /></JcProvider>);\n    expect(container).toBeTruthy();\n  });\n});";
/// The same page reaching for a package SDK-12 does not allow.
const STATIONS_AXIOS: &str = "import axios from \"axios\";\n\nexport function Stations() {\n  return <p>{String(axios)}</p>;\n}";
const APP_PAGES: &str = "      { id: \"overview\", label: \"Overview\", render: () => <Overview schema={schema} /> },\n";
const APP_IMPORTS: &str = "import { TypePage } from \"./pages/TypePage\";\n";

/// The blocks of an application with a Stations page wired into `App.tsx`.
fn stations_app(page: &str) -> Vec<(&'static str, &'static str, String)> {
    vec![
        ("src/pages/Stations.tsx", "", page.to_owned()),
        ("src/pages/Stations.test.tsx", "", STATIONS_TEST.to_owned()),
        (
            "src/App.tsx",
            APP_IMPORTS,
            format!(
                "{}import {{ Stations }} from \"./pages/Stations\";",
                APP_IMPORTS.trim_end()
            ),
        ),
        (
            "src/App.tsx",
            APP_PAGES,
            format!(
                "{}      {{ id: \"stations\", label: \"Stations\", render: () => <Stations /> }},",
                APP_PAGES
            ),
        ),
    ]
}

fn code_answer(prose: &str, blocks: &[(&str, &str, String)]) -> String {
    let blocks: Vec<(&str, &str, &str)> = blocks
        .iter()
        .map(|(path, search, replace)| (*path, *search, replace.as_str()))
        .collect();
    answer(prose, &blocks)
}

/// The endpoint's schema surface through the proxy and Model Tools' `/generate` (SDK-10).
async fn mount_types(proxy: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/v1/data/schema/index.json"))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "endpoint": SLUG,
            "models": [{ "name": "bikes", "version": 1, "types": ["BikeHireDockingStation"], "artifacts": {} }]
        })))
        .mount(proxy)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/data/schema/v1/model.linkml.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(BIKES_LINKML))
        .mount(proxy)
        .await;
    Mock::given(method("POST"))
        .and(path("/model-tools/generate"))
        .and(wiremock::matchers::body_json(
            json!({ "source": BIKES_LINKML }),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "typescript": JC_TYPES, "errors": [] })),
        )
        .mount(proxy)
        .await;
}

/// A run of the default kind, `application`.
async fn create_application(app: &axum::Router, cookie: &str) -> String {
    create_application_of(app, cookie, json!(["BikeHireDockingStation"])).await
}

/// An `application` run whose data needs name `types`.
async fn create_application_of(app: &axum::Router, cookie: &str, types: Value) -> String {
    let (status, body) = json(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointName": "helsinki-bikes",
            "appClass": "static",
            "visibility": "project",
            "prompt": "A page listing the bike stations",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": types,
                "attrs": ["name", "location", "availableBikeNumber"],
                "operations": ["queryEntity"]
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    body["id"].as_str().expect("a run id").to_owned()
}

/// The run once its preview is version `v`, or a panic after ten seconds.
async fn wait_for_version(app: &axum::Router, cookie: &str, id: &str, v: u32) -> Value {
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}");
    let mut last = Value::Null;
    for _ in 0..200 {
        let (_, run) = json(app, cookie, Method::GET, &uri, None).await;
        if run["previewUrl"]
            .as_str()
            .is_some_and(|url| url.ends_with(&format!("?v={v}")))
            && run["status"] == json!("previewing")
        {
            return run;
        }
        last = run;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the run never showed version {v}: {last}");
}

async fn files_of(state: &AppState, id: &str) -> Value {
    state
        .agents
        .get_run(id)
        .await
        .expect("the store answers")
        .expect("the run")
        .files
}

fn template_file(path: &str) -> String {
    joinedcontext_portal::agents::preview::template_files()
        .remove(path)
        .unwrap_or_else(|| panic!("{path} is in the template"))
}

/// A forge that takes one commit of many files (SDK-17).
async fn code_forge() -> MockServer {
    let forge = forge().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/org/manifests/contents"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({ "files": [], "commit": { "sha": "c0de" } })),
        )
        .mount(&forge)
        .await;
    forge
}

#[tokio::test]
async fn a_type_the_endpoint_does_not_serve_never_reaches_the_chat_the_samples_or_the_model() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (_state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    // `Entity` is the model's abstract base class: the JSON Schema defines it, the endpoint's
    // index serves only `BikeHireDockingStation`.
    let id =
        create_application_of(&app, &cookie, json!(["BikeHireDockingStation", "Entity"])).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    let log = events(&app, &cookie, &id).await;
    let reading: Vec<&str> = log
        .iter()
        .filter(|(kind, _)| kind == "thought")
        .filter_map(|(_, payload)| payload["text"].as_str())
        .filter(|text| text.starts_with("Reading"))
        .collect();
    assert_eq!(
        reading,
        ["Reading 5 entities of BikeHireDockingStation through the endpoint."]
    );

    let samples: Vec<String> = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/v1/data/ngsi-ld/v1/entities")
        .map(|r| r.url.query().unwrap_or_default().to_owned())
        .collect();
    assert!(!samples.is_empty(), "the served type was sampled");
    assert!(
        samples.iter().all(|query| !query.contains("type=Entity")),
        "{samples:?}"
    );

    let requests = model_requests(&proxy).await;
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    let needs = user
        .split("Data needs (types and attributes the person asked for):")
        .nth(1)
        .and_then(|rest| rest.split("```").nth(1))
        .expect("the data needs block");
    assert!(needs.contains("\"BikeHireDockingStation\""), "{needs}");
    assert!(!needs.contains("\"Entity\""), "{needs}");
}

/// The indicator space of the city: a second endpoint an application may read (AP-44).
const KPI_SLUG: &str = "q3mzkq2v7w5ayxcbn4ltdj6hofkpis";
const KPI_LINKML: &str = "id: https://hel.fi/models/kpis\nname: kpis\nclasses:\n  KeyPerformanceIndicator:\n    slots: [id, name, kpiValue]\nslots:\n  id: {}\n  name: { range: string }\n  kpiValue: { range: float }\n";
const KPI_TYPES: &str = "export interface KeyPerformanceIndicator { id: string; type: \"KeyPerformanceIndicator\"; name?: string; kpiValue?: number }\n";

/// AP-44, SDK-10, SDK-13: an application of the bikes and their indicators samples each type
/// through the endpoint that serves it, renders the row types of both models into one file and
/// tells the model which endpoint serves what.
#[tokio::test]
async fn an_application_of_two_endpoints_reads_each_through_its_own_and_the_pack_names_both() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    state.mirror.upsert(envelope(
        "Endpoint",
        "helsinki-kpi",
        PROJECT,
        json!({
            "slug": KPI_SLUG,
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-kpi" },
            "audience": "project"
        }),
    ));
    let kpis = format!("/v1/data/endpoints/{KPI_SLUG}");
    Mock::given(method("GET"))
        .and(path(format!("{kpis}/schema/index.json")))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "endpoint": KPI_SLUG,
            "models": [{ "name": "kpis", "version": 1, "types": ["KeyPerformanceIndicator"], "artifacts": {} }]
        })))
        .mount(&proxy)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{kpis}/ngsi-ld/v1/entities")))
        .and(query_param("type", "KeyPerformanceIndicator"))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:bikes-available-avg", "type": "KeyPerformanceIndicator", "name": "bikes-available-avg", "kpiValue": 4.2 }
        ])))
        .mount(&proxy)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{kpis}/schema/v1/model.linkml.yaml")))
        .respond_with(ResponseTemplate::new(200).set_body_string(KPI_LINKML))
        .mount(&proxy)
        .await;
    Mock::given(method("POST"))
        .and(path("/model-tools/generate"))
        .and(wiremock::matchers::body_json(
            json!({ "source": KPI_LINKML }),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "typescript": KPI_TYPES, "errors": [] })),
        )
        .mount(&proxy)
        .await;

    let (status, body) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointNames": ["helsinki-bikes", "helsinki-kpi"],
            "appClass": "static",
            "visibility": "project",
            "prompt": "The stations beside the availability indicator",
            "dataNeeds": [
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                    "types": ["BikeHireDockingStation"],
                    "attrs": ["name", "location", "availableBikeNumber"],
                    "operations": ["queryEntity"]
                },
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-kpi" },
                    "types": ["KeyPerformanceIndicator"],
                    "attrs": ["name", "kpiValue"],
                    "operations": ["queryEntity"]
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().expect("a run id").to_owned();
    wait_for_version(&app, &cookie, &id, 1).await;

    let paths: Vec<String> = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| format!("{}?{}", r.url.path(), r.url.query().unwrap_or_default()))
        .collect();
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with("/v1/data/ngsi-ld/v1/entities?type=BikeHireDockingStation")),
        "{paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.starts_with(&format!(
            "{kpis}/ngsi-ld/v1/entities?type=KeyPerformanceIndicator"
        ))),
        "{paths:?}"
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.starts_with("/v1/data/ngsi-ld/v1/entities?type=KeyPerformanceIndicator")),
        "the indicator is never read through the bikes endpoint: {paths:?}"
    );

    let requests = model_requests(&proxy).await;
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(user.contains("## THE ENDPOINTS"), "{user}");
    assert!(
        user.contains(
            "`helsinki-kpi` — context space `helsinki-kpi`, types: KeyPerformanceIndicator"
        ),
        "the pack says which endpoint serves the indicator"
    );
    assert!(
        user.contains("bikes-available-avg"),
        "the indicator's sample is in the pack"
    );

    let files = files_of(&state, &id).await;
    let types = files["src/jc-types.ts"].as_str().unwrap_or_default();
    assert!(
        types.contains("interface BikeHireDockingStation"),
        "{types}"
    );
    assert!(
        types.contains("interface KeyPerformanceIndicator"),
        "{types}"
    );
}

/// Operations beside the public stations, on the same space `helsinki`.
const OPS_SLUG: &str = "p8vx2kq7w5ayxcbn4ltdj6hofkops";

/// SDK-02, SDK-13, AP-44: the stations of the public endpoint and the status of the operations
/// endpoint, one space. The run samples the type through both, asks the second for the entities
/// the first answered, joins the rows by id, says in the chat which endpoints it read and tells the
/// model which endpoint carries which attributes.
#[tokio::test]
async fn a_type_two_endpoints_of_one_space_serve_is_sampled_through_both_and_joined_by_id() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    state.mirror.upsert(envelope(
        "Endpoint",
        "helsinki-bikes-ops",
        PROJECT,
        json!({
            "slug": OPS_SLUG,
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "audience": "project"
        }),
    ));
    let ops = format!("/v1/data/endpoints/{OPS_SLUG}");
    Mock::given(method("GET"))
        .and(path(format!("{ops}/schema/index.json")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "endpoint": OPS_SLUG,
            "models": [{ "name": "bikes", "version": 1, "types": ["BikeHireDockingStation"], "artifacts": {} }]
        })))
        .mount(&proxy)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ops}/schema/v1/model.linkml.yaml")))
        .respond_with(ResponseTemplate::new(200).set_body_string(BIKES_LINKML))
        .mount(&proxy)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ops}/ngsi-ld/v1/entities")))
        .and(query_param("type", "BikeHireDockingStation"))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "urn:ngsi-ld:BikeHireDockingStation:001", "type": "BikeHireDockingStation", "status": "outOfService" },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:002", "type": "BikeHireDockingStation", "status": "working" }
        ])))
        .mount(&proxy)
        .await;

    let (status, body) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointNames": ["helsinki-bikes", "helsinki-bikes-ops"],
            "appClass": "static",
            "visibility": "project",
            "prompt": "The stations with their status on a map",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "location", "status"],
                "operations": ["queryEntity"]
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().expect("a run id").to_owned();
    wait_for_version(&app, &cookie, &id, 1).await;

    let asked: Vec<String> = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == format!("{ops}/ngsi-ld/v1/entities"))
        .map(|r| {
            r.url
                .query_pairs()
                .find(|(key, _)| key == "id")
                .map(|(_, ids)| ids.into_owned())
                .unwrap_or_default()
        })
        .collect();
    assert!(
        asked
            .iter()
            .any(|ids| ids.contains("urn:ngsi-ld:BikeHireDockingStation:001")),
        "the operations endpoint is asked for the stations already read: {asked:?}"
    );

    let log = events(&app, &cookie, &id).await;
    assert!(
        log.iter().any(|(kind, payload)| kind == "thought"
            && payload["text"]
                == json!("Reading 5 entities of BikeHireDockingStation through helsinki-bikes and helsinki-bikes-ops.")),
        "{log:?}"
    );

    let requests = model_requests(&proxy).await;
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    let samples = user
        .split("Five entities per type, as the SDK's rows (`options=keyValues`):")
        .nth(1)
        .and_then(|rest| rest.split("```").nth(1))
        .and_then(|block| serde_json::from_str::<Value>(block.trim_start_matches("json")).ok())
        .expect("the samples block");
    let first = &samples["BikeHireDockingStation"][0];
    assert_eq!(first["name"], json!("Kaivopuisto"), "{samples}");
    assert_eq!(first["status"], json!("outOfService"), "{samples}");
    assert!(
        user.contains("Types read from several endpoints:"),
        "{user}"
    );
    assert!(
        user.contains("`helsinki-bikes-ops` carries status"),
        "{user}"
    );
    assert!(
        user.contains("join the rows by `id`: BikeHireDockingStation"),
        "{user}"
    );
}

#[tokio::test]
async fn an_application_is_written_in_one_call_and_the_template_never_reaches_the_frame() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    let run = wait_for_version(&app, &cookie, &id, 1).await;
    assert!(run["firstVersionMs"].is_i64(), "{run}");
    // The first frame is the first generated version, so both mark the same moment.
    assert!(
        run["firstFrameMs"].as_i64() <= run["firstVersionMs"].as_i64()
            && run["firstFrameMs"].is_i64(),
        "{run}"
    );

    // The template is the model's context, never the preview: the first preview is the
    // generated version, after the building status (SDK-14, SDK-15).
    let log = events(&app, &cookie, &id).await;
    let previews: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, (kind, _))| kind == "preview")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(previews.len(), 1, "{log:?}");
    let building = log
        .iter()
        .position(|(kind, payload)| kind == "status" && payload["status"] == json!("building"))
        .expect("the building status");
    assert!(previews[0] > building, "{log:?}");
    assert!(log[previews[0]].1["previewUrl"]
        .as_str()
        .is_some_and(|url| url.ends_with("?v=1")));
    assert!(log.iter().any(|(kind, payload)| kind == "thought"
        && payload["text"] == json!("Writing the application for your request.")));
    assert!(!log.iter().any(|(_, payload)| payload["text"]
        .as_str()
        .is_some_and(|t| t.contains("template"))));

    // A small first version on the code prompt, with the SDK, every template file and the
    // endpoint's types, then the call that completes it (SDK-13).
    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["max_tokens"], json!(20000));
    assert!(requests[0]["messages"][1]["content"]
        .as_str()
        .is_some_and(|user| user.contains("FIRST VERSION")));
    assert_eq!(requests[1]["max_tokens"], json!(64000));
    assert!(requests[1]["messages"][1]["content"]
        .as_str()
        .is_some_and(|user| user.contains("Complete the application for the request")));
    let system = requests[0]["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(system.contains("AN APPLICATION ON THE JOINEDCONTEXT APP SDK"));
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    for part in [
        "## THE SDK",
        "function useEntities",
        "### src/components/EntityTable.tsx",
        "### functions/summary.test.ts",
        "### package.json",
        JC_TYPES.trim(),
        "Kaivopuisto",
        "may NOT write",
        "A page listing the bike stations",
    ] {
        assert!(user.contains(part), "the pack lacks {part}");
    }

    // The files: the template, the rendered types, the page and its test, the wiring.
    let files = files_of(&state, &id).await;
    assert_eq!(files["src/jc-types.ts"], json!(JC_TYPES));
    assert_eq!(
        files["src/pages/Stations.tsx"],
        json!(format!("{STATIONS}\n"))
    );
    assert!(files["src/pages/Stations.test.tsx"].is_string());
    let app_tsx = files["src/App.tsx"].as_str().unwrap_or_default();
    assert!(
        app_tsx.contains("import { Stations } from \"./pages/Stations\";"),
        "{app_tsx}"
    );
    assert!(app_tsx.contains("<Stations />"), "{app_tsx}");
    assert_eq!(files["package.json"], json!(template_file("package.json")));
    let tool = log
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == json!("apply_patch"))
        .expect("the apply_patch event");
    assert_eq!(
        tool.1["applied"].as_array().map(Vec::len),
        Some(4),
        "{tool:?}"
    );

    // One commit on the run branch with the whole project under the application's folder.
    let commits: Vec<Value> = forge
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| {
            r.method.as_str() == "POST" && r.url.path() == "/api/v1/repos/org/manifests/contents"
        })
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect();
    assert_eq!(commits.len(), 1, "{commits:?}");
    let paths: Vec<&str> = commits[0]["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str())
        .collect();
    for path in [
        "projects/helsinki/apps/city-bikes-overview/src/pages/Stations.tsx",
        "projects/helsinki/apps/city-bikes-overview/src/jc-types.ts",
        "projects/helsinki/apps/city-bikes-overview/package.json",
    ] {
        assert!(paths.contains(&path), "{path} not in {paths:?}");
    }
    assert!(log
        .iter()
        .any(|(kind, payload)| kind == "commit" && payload["sha"] == json!("c0de")));

    // The preview is the code document, or 503 in a build without the SDK runtime.
    let (status, headers, body) = call(
        &app,
        &cookie,
        Method::GET,
        run["previewUrl"].as_str().expect("a preview url"),
        None,
    )
    .await;
    match status {
        StatusCode::OK => {
            assert!(String::from_utf8_lossy(&body).contains("importmap"));
            assert!(headers.contains_key(header::CONTENT_SECURITY_POLICY));
        }
        StatusCode::SERVICE_UNAVAILABLE => {}
        other => panic!(
            "the preview answered {other}: {}",
            String::from_utf8_lossy(&body)
        ),
    }
}

#[tokio::test]
async fn a_block_outside_the_writable_paths_is_refused_and_the_rest_lands() {
    let mut blocks = stations_app(STATIONS);
    blocks.push(("src/main.tsx", "", "console.log(\"mine\");".to_owned()));
    blocks.push(("package.json", "", "{}".to_owned()));
    let (state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[code_answer("Stations.", &blocks)],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    let log = events(&app, &cookie, &id).await;
    let tool = log
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == json!("apply_patch"))
        .expect("the apply_patch event");
    assert_eq!(tool.1["exitCode"], json!(1));
    let refused: Vec<&str> = tool.1["refused"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["path"].as_str())
        .collect();
    assert_eq!(refused, ["src/main.tsx", "package.json"]);
    assert!(tool.1["refused"][0]["reason"]
        .as_str()
        .is_some_and(|reason| reason.contains("never src/main.tsx")));
    // A refusal alone is on the log, not a repair (SDK-11).
    assert_eq!(model_requests(&proxy).await.len(), 2);
    let files = files_of(&state, &id).await;
    assert_eq!(files["src/main.tsx"], json!(template_file("src/main.tsx")));
    assert_eq!(files["package.json"], json!(template_file("package.json")));
    assert!(files["src/pages/Stations.tsx"].is_string());
}

#[tokio::test]
async fn a_refused_import_goes_back_once_with_its_file_and_line_and_the_repair_lands() {
    let (state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[
            code_answer("Stations.", &stations_app(STATIONS_AXIOS)),
            code_answer(
                "Stations without axios.",
                &[("src/pages/Stations.tsx", "", STATIONS.to_owned())],
            ),
        ],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 3);
    let repair = requests[1]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(repair.contains("does not build"), "{repair}");
    assert!(
        repair.contains("- src/pages/Stations.tsx:1:"),
        "the repair names file and line"
    );
    assert!(repair.contains("axios"));
    let files = files_of(&state, &id).await;
    assert_eq!(
        files["src/pages/Stations.tsx"],
        json!(format!("{STATIONS}\n"))
    );
    let log = events(&app, &cookie, &id).await;
    assert!(log.iter().any(|(kind, payload)| {
        kind == "thought"
            && payload["text"].as_str().is_some_and(|t| {
                t.starts_with("The application does not build; asking for a repair")
            })
    }));
}

#[tokio::test]
async fn a_second_failure_ends_the_first_run_with_no_preview_and_the_errors_said() {
    let (state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[
            code_answer("Stations.", &stations_app(STATIONS_AXIOS)),
            code_answer("Still axios.", &stations_app(STATIONS_AXIOS)),
        ],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    let run = wait_for(&app, &cookie, &id, &["previewing", "failed"]).await;
    assert_eq!(run["status"], json!("previewing"), "{run}");
    // Neither the template nor the version that failed is put in the frame (SDK-14, AP-59).
    assert!(run["previewUrl"].is_null(), "{run}");
    assert!(run.get("firstVersionMs").is_none(), "{run}");

    assert_eq!(model_requests(&proxy).await.len(), 2);
    let files = files_of(&state, &id).await;
    assert!(files.get("src/pages/Stations.tsx").is_none());
    assert_eq!(files["src/App.tsx"], json!(template_file("src/App.tsx")));
    assert_eq!(files["src/jc-types.ts"], json!(JC_TYPES));
    let log = events(&app, &cookie, &id).await;
    assert!(log.iter().any(|(kind, payload)| kind == "thought"
        && payload["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("The application could not be built")
                && t.contains("axios")
                && t.ends_with("Send a message to try again."))));
    assert!(!log.iter().any(|(kind, _)| kind == "preview"), "{log:?}");
}

/// Posts what the frame saw of version `v`, as the host page relays it (SDK-27).
async fn observe(app: &axum::Router, cookie: &str, id: &str, v: u32, pages: Value) -> StatusCode {
    let (status, _, _) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-observations"),
        Some(json!({ "version": v, "pages": pages })),
    )
    .await;
    status
}

/// The run's first thought starting with `prefix`, or a panic after ten seconds.
async fn wait_for_thought(app: &axum::Router, cookie: &str, id: &str, prefix: &str) -> String {
    for _ in 0..200 {
        if let Some(text) = events(app, cookie, id)
            .await
            .into_iter()
            .filter(|(kind, _)| kind == "thought")
            .filter_map(|(_, payload)| payload["text"].as_str().map(str::to_owned))
            .find(|text| text.starts_with(prefix))
        {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no thought starting with {prefix:?}");
}

#[tokio::test]
async fn a_version_whose_pages_show_the_sampled_entities_is_checked_without_a_model_call() {
    let (_state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[code_answer("Stations.", &stations_app(STATIONS))],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    let status = observe(
        &app,
        &cookie,
        &id,
        1,
        json!([
            { "label": "Overview", "text": "Stations 5 · Bikes available 24", "rows": [] },
            { "label": "Stations", "text": "Kaivopuisto 7 Laivasillankatu 2 Viiskulma 11", "rows": [5] }
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let said = wait_for_thought(&app, &cookie, &id, "Checked").await;
    assert_eq!(
        said,
        "Checked 2 pages: 3 of 5 sampled entities shown, no errors."
    );
    assert_eq!(model_requests(&proxy).await.len(), 2);
}

#[tokio::test]
async fn what_the_preview_shows_wrong_goes_back_as_a_verification_pass_and_a_new_version() {
    let repaired = STATIONS.replace("{rows.map", "{(rows ?? []).map");
    let (state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[
            code_answer("Stations.", &stations_app(STATIONS)),
            code_answer("Nothing to add.", &[]),
            code_answer(
                "Rows may be absent.",
                &[("src/pages/Stations.tsx", "", repaired.clone())],
            ),
        ],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    // The runtime error the frame reported is folded into the check of the same version.
    let (status, _, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-errors"),
        Some(
            json!({ "message": "rows is undefined", "file": "src/pages/Stations.tsx", "line": 6 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = observe(
        &app,
        &cookie,
        &id,
        1,
        json!([{ "label": "Overview", "text": "Stations NaN", "rows": [] }]),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    wait_for_version(&app, &cookie, &id, 2).await;

    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 3);
    let pass = requests[2]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    for part in [
        "Checking its preview against the data found the problems below",
        "- the preview threw: rows is undefined (src/pages/Stations.tsx:6)",
        "- the page \"Overview\" shows NaN where a value belongs",
        "- no page shows any of the sampled BikeHireDockingStation entities (Kaivopuisto, Laivasillankatu, Kapteeninpuistikko)",
        "What the preview rendered, page by page:\n### Overview\nStations NaN",
    ] {
        assert!(pass.contains(part), "the pass lacks {part}: {pass}");
    }
    assert_eq!(
        files_of(&state, &id).await["src/pages/Stations.tsx"],
        json!(format!("{repaired}\n"))
    );
    let found = wait_for_thought(&app, &cookie, &id, "Checking the preview found:").await;
    assert!(found.contains("shows NaN"), "{found}");
}

#[tokio::test]
async fn verification_stops_after_three_passes_and_says_what_is_left() {
    let page = |n: u32| STATIONS.replace("<ul>", &format!("<ul data-pass=\"{n}\">"));
    let (_state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[
            code_answer("Stations.", &stations_app(STATIONS)),
            code_answer("Nothing to add.", &[]),
            code_answer("One.", &[("src/pages/Stations.tsx", "", page(1))]),
            code_answer("Two.", &[("src/pages/Stations.tsx", "", page(2))]),
            code_answer("Three.", &[("src/pages/Stations.tsx", "", page(3))]),
        ],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    let broken = json!([{ "label": "Stations", "text": "undefined", "rows": [0] }]);
    for v in 1..=3 {
        wait_for_version(&app, &cookie, &id, v).await;
        assert_eq!(
            observe(&app, &cookie, &id, v, broken.clone()).await,
            StatusCode::NO_CONTENT
        );
    }
    wait_for_version(&app, &cookie, &id, 4).await;
    assert_eq!(
        observe(&app, &cookie, &id, 4, broken).await,
        StatusCode::NO_CONTENT
    );
    let left = wait_for_thought(&app, &cookie, &id, "The preview still shows problems").await;
    assert!(left.contains("after 3 verification passes"), "{left}");
    assert!(left.contains("shows undefined"), "{left}");
    assert_eq!(model_requests(&proxy).await.len(), 5);
}

// ---- The editing agent: every message after the first version is a tool loop (T-0684, SDK-20) ----

/// One model answer that calls the tools, in the provider's body.
fn tool_answer(provider: &str, calls: &[(&str, &str, Value)]) -> Value {
    if provider == "anthropic" {
        let content: Vec<Value> = calls
            .iter()
            .map(|(id, name, input)| {
                json!({ "type": "tool_use", "id": id, "name": name, "input": input })
            })
            .collect();
        json!({
            "id": "msg_tools",
            "type": "message",
            "role": "assistant",
            "content": content,
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 300, "output_tokens": 40 }
        })
    } else {
        let tool_calls: Vec<Value> = calls
            .iter()
            .map(|(id, name, input)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": input.to_string() }
                })
            })
            .collect();
        json!({
            "choices": [{
                "message": { "role": "assistant", "content": null, "tool_calls": tool_calls },
                "finish_reason": "tool_calls"
            }],
            "usage": { "total_tokens": 340 }
        })
    }
}

/// The model route of `provider`, answering `bodies` in order, one each.
async fn mount_tool_answers(proxy: &MockServer, provider: &str, bodies: &[Value]) {
    let route = if provider == "anthropic" {
        "/v1/llm/messages"
    } else {
        "/v1/llm/chat/completions"
    };
    for body in bodies {
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .up_to_n_times(1)
            .mount(proxy)
            .await;
    }
}

async fn send_message(app: &axum::Router, cookie: &str, id: &str, text: &str) {
    let (status, body) = json(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": text })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// The `tool` events of the run as (tool, status, result), in order.
fn tool_events(log: &[(String, Value)]) -> Vec<(String, String, String)> {
    // The loop's events carry their step; the first pass's `apply_patch` has none.
    log.iter()
        .filter(|(kind, payload)| kind == "tool" && payload.get("step").is_some())
        .map(|(_, payload)| {
            (
                payload["tool"].as_str().unwrap_or_default().to_owned(),
                payload["status"].as_str().unwrap_or_default().to_owned(),
                payload["result"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// A message after the first version: read, an edit that reaches for axios, a check that
/// refuses it, the fix, a passing check that reloads the frame, finish (SDK-20, SDK-12).
async fn a_message_after_the_first_version_is_a_tool_loop(provider: &str) {
    let forge = code_forge().await;
    // The first version, the completing pass (unchanged), then the six tool turns: the proxy
    // answers in mount order, so everything is mounted before the run starts.
    let first = code_answer("A page listing the stations.", &stations_app(STATIONS));
    let complete = code_answer("Complete.", &[]);
    let (state, app, cookie, proxy) =
        portal_state_with(provider, &[first, complete], Some(&forge)).await;
    mount_types(&proxy).await;
    mount_tool_answers(
        &proxy,
        provider,
        &[
            tool_answer(provider, &[("t1", "read_file", json!({ "path": "src/pages/Stations.tsx" }))]),
            tool_answer(
                provider,
                &[(
                    "t2",
                    "edit_file",
                    json!({ "path": "src/pages/Stations.tsx", "search": "", "replace": STATIONS_AXIOS }),
                )],
            ),
            tool_answer(provider, &[("t3", "check", json!({}))]),
            tool_answer(
                provider,
                &[(
                    "t4",
                    "edit_file",
                    json!({ "path": "src/pages/Stations.tsx", "search": "", "replace": STATIONS }),
                )],
            ),
            tool_answer(provider, &[("t5", "check", json!({}))]),
            tool_answer(
                provider,
                &[("t6", "finish", json!({ "message": "The Stations page reads through the SDK again." }))],
            ),
        ],
    )
    .await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;
    send_message(&app, &cookie, &id, "Fetch the stations with axios").await;
    let run = wait_for_version(&app, &cookie, &id, 2).await;
    assert!(
        run["previewUrl"]
            .as_str()
            .unwrap_or_default()
            .ends_with("?v=2"),
        "one passing check after a change is one reload: {run}"
    );

    let log = events(&app, &cookie, &id).await;
    let tools = tool_events(&log);
    let names: Vec<&str> = tools.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "read_file",
            "edit_file",
            "check",
            "edit_file",
            "check",
            "finish"
        ],
        "{tools:?}"
    );
    assert_eq!(tools[0].1, "ok");
    assert!(
        tools[0].2.contains("useEntities"),
        "the read shows the file: {}",
        tools[0].2
    );
    assert_eq!(tools[2].1, "failed");
    assert!(
        tools[2].2.contains("axios"),
        "the check names the refused import: {}",
        tools[2].2
    );
    assert_eq!(
        tools[4],
        ("check".to_owned(), "ok".to_owned(), "ok".to_owned())
    );
    let files = files_of(&state, &id).await;
    assert_eq!(
        files["src/pages/Stations.tsx"].as_str().map(str::trim_end),
        Some(STATIONS.trim_end())
    );
    assert!(
        log.iter().any(|(kind, payload)| kind == "thought"
            && payload["text"] == json!("The Stations page reads through the SDK again.")),
        "the finish message reaches the chat"
    );
    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 8, "two first-run passes and six tool turns");
    let last = requests[7].to_string();
    assert!(
        last.contains("\"tools\""),
        "the tools travel in the body: {last}"
    );
    assert!(last.contains("Fetch the stations with axios"), "{last}");
}

#[tokio::test]
async fn a_message_after_the_first_version_is_a_tool_loop_for_anthropic() {
    a_message_after_the_first_version_is_a_tool_loop("anthropic").await;
}

#[tokio::test]
async fn a_message_after_the_first_version_is_a_tool_loop_for_openai() {
    a_message_after_the_first_version_is_a_tool_loop("openai-compatible").await;
}

/// A write outside SDK-11's paths is a refused tool result, never a write and never the end
/// of the turn.
#[tokio::test]
async fn a_write_outside_the_sdk_paths_is_refused_as_a_tool_result() {
    let forge = code_forge().await;
    let first = code_answer("A page listing the stations.", &stations_app(STATIONS));
    let complete = code_answer("Complete.", &[]);
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[first, complete], Some(&forge)).await;
    mount_types(&proxy).await;
    mount_tool_answers(
        &proxy,
        "openai-compatible",
        &[
            tool_answer(
                "openai-compatible",
                &[
                    (
                        "w1",
                        "write_file",
                        json!({ "path": "../../etc/passwd", "content": "root" }),
                    ),
                    (
                        "w2",
                        "write_file",
                        json!({ "path": "package.json", "content": "{}" }),
                    ),
                ],
            ),
            tool_answer(
                "openai-compatible",
                &[("f", "finish", json!({ "message": "Nothing to change." }))],
            ),
        ],
    )
    .await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;
    wait_for_thought(&app, &cookie, &id, "Complete.").await;
    let before = files_of(&state, &id).await;
    send_message(&app, &cookie, &id, "Write the password file").await;
    let said = wait_for_thought(&app, &cookie, &id, "Nothing to change.").await;
    assert_eq!(said, "Nothing to change.");

    let tools = tool_events(&events(&app, &cookie, &id).await);
    assert_eq!(tools.len(), 3, "{tools:?}");
    for refused in &tools[..2] {
        assert_eq!(refused.0, "write_file");
        assert_eq!(refused.1, "failed");
        assert!(
            refused
                .2
                .starts_with("not a path the application may write"),
            "{}",
            refused.2
        );
    }
    assert_eq!(files_of(&state, &id).await, before, "nothing was written");
}

/// The profile's `stepsPerRun` bounds one message: the turn ends with a word for the person
/// and no further model call.
#[tokio::test]
async fn the_step_limit_ends_the_turn_with_a_message() {
    let forge = code_forge().await;
    let first = code_answer("A page listing the stations.", &stations_app(STATIONS));
    let complete = code_answer("Complete.", &[]);
    let (state, app, cookie, proxy) =
        portal_state_with("anthropic", &[first, complete], Some(&forge)).await;
    mount_types(&proxy).await;
    state.mirror.upsert(envelope(
        "AgentProfile",
        "app-builder",
        "org",
        json!({
            "role": "builder",
            "runtime": {
                "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            },
            "model": { "provider": "anthropic", "name": "claude-sonnet-5", "maxTokensPerRun": 400000, "reasoningEffort": "medium" },
            "limits": { "stepsPerRun": 3, "wallClock": "PT20M", "concurrentRunsPerOrganization": 2, "requestsPerMinute": 60, "maxResponseBytes": 2097152 },
            "egress": { "allowedHosts": [] },
            "tools": ["shell"],
            "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
        }),
    ));
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;
    wait_for_thought(&app, &cookie, &id, "Complete.").await;

    // The model never stops asking for the file list.
    Mock::given(method("POST"))
        .and(path("/v1/llm/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(tool_answer("anthropic", &[("l", "list_files", json!({}))])),
        )
        .mount(&proxy)
        .await;
    send_message(&app, &cookie, &id, "Look around").await;
    let said = wait_for_thought(&app, &cookie, &id, "The step limit").await;
    assert!(said.contains("3 tool calls"), "{said}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let tools = tool_events(&events(&app, &cookie, &id).await);
    assert_eq!(tools.len(), 3, "{tools:?}");
    assert_eq!(
        model_requests(&proxy).await.len(),
        2 + 3,
        "two first-run passes, three tool turns, no call past the limit"
    );
}
