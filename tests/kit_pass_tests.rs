//! The kit pass through the real router (T-0571, AP-56…AP-60, AG-53, AG-54, UI-41).
//!
//! A `static` run is driven by the Portal itself: samples through the proxy, one model call,
//! SEARCH/REPLACE blocks applied to `spec.json`, a preview document. The proxy is a wiremock
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
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn config(proxy_base: &str) -> Config {
    Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some(proxy_base),
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
        "AgentProfile",
        "app-builder",
        "org",
        json!({
            "role": "builder",
            "runtime": {
                "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            },
            "model": { "provider": provider, "name": "claude-sonnet-5", "maxTokensPerRun": 400000 },
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
    let app = server::app(AppState::new(config.clone(), None).with_mirror(mirror(provider)));
    let cookie = session_cookie(&config, STEWARD);
    (app, cookie, proxy)
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
            "prompt": "Create a live bike availability dashboard with station filtering",
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

    let run = wait_for(&app, &cookie, &id, &["previewing", "failed"]).await;
    assert_eq!(run["status"], json!("previewing"), "{run}");
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
        ["queued", "starting", "building", "testing", "previewing"]
    );
    let tool = log
        .iter()
        .find(|(kind, _)| kind == "tool")
        .expect("a tool event");
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

    // The preview: one document with its own policy, or 503 in a build without the bundle.
    let (status, headers, bytes) = call(&app, &cookie, Method::GET, &preview_url, None).await;
    match kit::bundle() {
        Some((js, _)) => {
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
            assert!(csp.contains(&kit::script_hash(&js)), "{csp}");
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
    let run = wait_for(&app, &cookie, &id, &["previewing", "failed"]).await;
    assert_eq!(run["status"], json!("previewing"), "{run}");

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
    let run = wait_for(&app, &cookie, &id, &["previewing", "failed"]).await;
    // Refused blocks are not a validation problem: the specification is there and valid, so
    // no repair is asked for and the preview is shown (AP-58).
    assert_eq!(run["status"], json!("previewing"), "{run}");
    let log = events(&app, &cookie, &id).await;
    let tool = log
        .iter()
        .find(|(kind, _)| kind == "tool")
        .expect("a tool event");
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
    let run = wait_for(&app, &cookie, &id, &["previewing", "failed"]).await;
    assert_eq!(run["status"], json!("previewing"), "{run}");
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
    assert_eq!(run["status"], json!("previewing"));

    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 2);
    let second = requests[1].to_string();
    assert!(second.contains("Call it Kaupunkipyörät"), "{second}");
    assert!(
        second.contains("### spec.json"),
        "the current file is in the pack: {second}"
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
    let run = wait_for(&app, &cookie, &id, &["failed", "previewing"]).await;
    assert_eq!(run["status"], json!("failed"), "{run}");
    assert!(
        run["error"]
            .as_str()
            .is_some_and(|e| e.contains("the proxy answered 404")),
        "{run}"
    );
    assert!(run["previewUrl"].is_null());
}
