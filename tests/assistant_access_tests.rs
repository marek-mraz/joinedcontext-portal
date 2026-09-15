//! What an assistant may do (T-0674, AG-70): the profile's access block narrows the tools, the
//! person who started the conversation narrows them again, and a call outside either is a
//! refused `tool` event before anything runs. The model is a stub proxy that answers every turn
//! with a share request; the cases differ only in the profile and the person.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::agents::run::AgentRunEvent;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF: &str = "test-csrf-token-access";
const SHARE_SECTION: &str = "## WHEN THE PERSON ASKS TO SHARE OR PUBLISH DATA";
const KPI_SECTION: &str = "## WHEN THE PERSON ASKS FOR AN INDICATOR";

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

fn person(email: &str, groups: &[&str]) -> Identity {
    Identity {
        subject: format!("f:1:{email}"),
        username: email.split('@').next().unwrap_or(email).to_owned(),
        email: Some(email.to_owned()),
        name: None,
        roles: Vec::new(),
        groups: groups.iter().map(|g| (*g).to_owned()).collect(),
    }
}

fn cookie(config: &Config, identity: Identity) -> String {
    let now = session::now_unix();
    let s = Session {
        identity,
        expires_at: now + 3600,
        issued_at: now,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, namespace),
        spec,
        status: None,
    }
}

/// The organization, the builder profile with `access` (or none), and a reader who may start a
/// conversation (propose an App) and nothing else.
fn mirror(access: Option<Value>) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" }),
    ));
    let mut profile = json!({
        "role": "builder",
        "runtime": {
            "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
            "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
        },
        "model": { "provider": "openai-compatible", "name": "deepseek/deepseek-v4.1-flash", "maxTokensPerRun": 400000 },
        "limits": { "stepsPerRun": 120, "wallClock": "PT20M", "concurrentRunsPerOrganization": 2, "requestsPerMinute": 60, "maxResponseBytes": 2097152 },
        "egress": { "allowedHosts": ["registry.npmjs.org"] },
        "tools": ["shell"],
        "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
    });
    if let Some(access) = access {
        profile["access"] = access;
    }
    mirror.upsert(envelope(
        "AgentProfile",
        "app-builder",
        ORG_NAMESPACE,
        profile,
    ));
    mirror.upsert(envelope(
        "Role",
        "app-starter",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App"], "verbs": ["propose"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "reader-starts-apps",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "reader@hel.fi" }],
            "role": "app-starter",
            "scope": { "project": "helsinki" }
        }),
    ));
    mirror
}

const SHARE_ANSWER: &str = "I will draft that endpoint.\n\n```json\n{\"tool\":\"propose_endpoint\",\"contextSpace\":\"helsinki\",\"name\":\"bikes-regional-transport\",\"title\":\"City bikes for regional transport\",\"audience\":\"project-list\",\"allowedProjects\":[\"regional-transport\"],\"representations\":[\"ngsi-ld\"],\"hiddenAttributes\":[],\"entityTypes\":[\"BikeHireDockingStation\"]}\n```\n";

/// Starts a conversation as `who` under a profile with `access`, and returns the state, the
/// run's `propose_endpoint` tool event once it is published, and every prompt the model was sent.
async fn converse(access: Option<Value>, who: Identity) -> (AppState, Value, String) {
    let (state, events, prompts) = converse_with(
        access,
        who,
        Conversation {
            answer: SHARE_ANSWER,
            message: "Share the city bikes with the regional transport team",
            tool: "propose_endpoint",
            seeded: Vec::new(),
        },
    )
    .await;
    let tool = events
        .iter()
        .find(|e| e.kind == "tool")
        .map(|e| e.payload.clone())
        .expect("a tool event");
    (state, tool, prompts)
}

/// What one conversation is made of: the model's one answer, the person's message, the tool
/// the run is waited on for, and the manifests the mirror holds beyond the organization's.
struct Conversation {
    answer: &'static str,
    message: &'static str,
    tool: &'static str,
    seeded: Vec<ResourceEnvelope>,
}

/// The run's events from the first `tool` event named `conversation.tool` on, once the run has
/// published it and everything after it the driver publishes in the same turn.
async fn converse_with(
    access: Option<Value>,
    who: Identity,
    conversation: Conversation,
) -> (AppState, Vec<AgentRunEvent>, String) {
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1", "object": "chat.completion",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": conversation.answer }, "finish_reason": "stop" }],
            "usage": { "total_tokens": 900 }
        })))
        .mount(&proxy)
        .await;
    let config = config(&proxy.uri());
    let mirror = mirror(access);
    for envelope in conversation.seeded {
        mirror.upsert(envelope);
    }
    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/helsinki/assistant/conversations")
                .header(header::COOKIE, cookie(&config, who))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "message": conversation.message }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let created: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let id = created["id"].as_str().expect("run id").to_owned();

    let mut turn = None;
    for _ in 0..200 {
        let events: Vec<AgentRunEvent> = state.agents.events_since(&id, 0).await.expect("events");
        if let Some(at) = events
            .iter()
            .position(|e| e.kind == "tool" && e.payload["tool"] == conversation.tool)
        {
            // The turn is over once the run is back to waiting for the person: a navigate or a
            // thought after the tool step is its last event.
            let after = &events[at..];
            if after.iter().any(|e| e.kind == "thought")
                && (after[0].payload["status"] == "failed"
                    || after.iter().any(|e| e.kind == "navigate"))
            {
                turn = Some(after.to_vec());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let events =
        turn.unwrap_or_else(|| panic!("the run published the {} tool event", conversation.tool));
    let prompts = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    (state, events, prompts)
}

#[tokio::test]
async fn a_tool_outside_the_profile_is_absent_from_the_prompt_and_refused_when_called() {
    let access = json!({ "operations": ["jc_catalog_search"], "kinds": [] });
    let (state, tool, prompts) =
        converse(Some(access), person("admin@hel.fi", &["portal-approver"])).await;

    assert!(!prompts.is_empty(), "the model was asked");
    assert!(
        !prompts.contains(SHARE_SECTION),
        "the share tool is not offered"
    );
    assert!(
        !prompts.contains("## WHEN THE PERSON ASKS TO CHANGE AN ENDPOINT"),
        "nor the edit tool, which is the same operation"
    );
    assert!(
        !prompts.contains(KPI_SECTION),
        "the KPI tool is not offered"
    );
    assert_eq!(tool["tool"], "propose_endpoint");
    assert_eq!(tool["status"], "failed");
    assert!(
        tool["error"]
            .as_str()
            .is_some_and(|e| e.contains("does not grant jc_endpoint_propose")),
        "{tool}"
    );
    let draft = state
        .drafts
        .get("helsinki", "Endpoint", "bikes-regional-transport")
        .await
        .expect("drafts");
    assert!(draft.is_none(), "nothing ran");
}

#[tokio::test]
async fn a_profile_that_grants_the_tool_never_widens_a_person_who_may_not_propose() {
    let access = json!({
        "operations": ["jc_catalog_search", "jc_endpoint_propose"],
        "kinds": [{ "kind": "Endpoint", "verbs": ["read", "propose"] }]
    });
    let (state, tool, prompts) = converse(Some(access), person("reader@hel.fi", &[])).await;

    assert!(
        !prompts.contains(SHARE_SECTION),
        "the person may not propose an Endpoint"
    );
    assert_eq!(tool["status"], "failed", "{tool}");
    let draft = state
        .drafts
        .get("helsinki", "Endpoint", "bikes-regional-transport")
        .await
        .expect("drafts");
    assert!(draft.is_none(), "a profile never widens the reader");
}

#[tokio::test]
async fn a_granted_tool_runs_as_the_person_who_started_the_conversation() {
    let access = json!({
        "operations": ["jc_catalog_search", "jc_endpoint_propose"],
        "kinds": [{ "kind": "Endpoint", "verbs": ["read", "propose"] }]
    });
    let (state, tool, prompts) =
        converse(Some(access), person("admin@hel.fi", &["portal-approver"])).await;

    assert!(prompts.contains(SHARE_SECTION));
    assert!(
        !prompts.contains(KPI_SECTION),
        "jc_kpi_compute is not named"
    );
    assert_eq!(tool["status"], "ok", "{tool}");
    let draft = state
        .drafts
        .get("helsinki", "Endpoint", "bikes-regional-transport")
        .await
        .expect("drafts")
        .expect("the share drafted the endpoint");
    assert_eq!(draft.touched_by, "admin");
}

#[tokio::test]
async fn a_profile_without_an_access_block_offers_only_read_only_tools() {
    let (_, tool, prompts) = converse(None, person("admin@hel.fi", &["portal-approver"])).await;

    // jc_endpoint_propose renders manifests and writes nothing, so it carries readOnlyHint.
    assert!(prompts.contains(SHARE_SECTION));
    assert!(!prompts.contains("## WHEN THE PERSON ASKS TO COMPLETE A CONTEXT SPACE"));
    assert_eq!(tool["status"], "ok", "{tool}");
}

const CHANGE_SECTION: &str = "## WHEN THE PERSON ASKS TO CHANGE OR REMOVE SOMETHING";

/// A profile that may open changes of pipelines and nothing else.
fn pipeline_access() -> Value {
    json!({
        "operations": ["jc_catalog_search", "jc_resource_propose"],
        "kinds": [{ "kind": "Pipeline", "verbs": ["read", "propose"] }]
    })
}

/// The project's context space, an endpoint into it and the hel-news pipeline that writes there.
fn news_pipeline() -> Vec<ResourceEnvelope> {
    let mut seeded = bikes_space();
    seeded.push(envelope(
        "Pipeline",
        "hel-news",
        "helsinki",
        json!({
            "class": "resident",
            "enabled": true,
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all",
            "quotas": { "maxMemoryMb": 128, "cpuMillicores": 250 }
        }),
    ));
    seeded
}

#[tokio::test]
async fn a_change_to_an_existing_endpoint_opens_its_form_with_the_change_and_keeps_its_slug() {
    let mut endpoint = envelope(
        "Endpoint",
        "helsinki-news",
        "helsinki",
        json!({
            "contextSpaceRef": "helsinki",
            "slug": "newsnewsnewsnewsnewsnewsne",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld", "geojson"]
        }),
    );
    endpoint
        .metadata
        .labels
        .insert("joinedcontext.com/space".into(), "helsinki".into());
    let mut seeded = bikes_space();
    seeded.push(endpoint);
    let (state, events, prompts) = converse_with(
        None,
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: "I will add CSV to the news endpoint.\n\n```json\n{\"tool\":\"change_resource\",\"kind\":\"Endpoint\",\"name\":\"helsinki-news\",\"patch\":{\"spec\":{\"enabledRepresentations\":[\"ngsi-ld\",\"geojson\",\"csv\"]}}}\n```\n",
            message: "Add csv to helsinki-news",
            tool: "change_resource",
            seeded,
        },
    )
    .await;

    assert!(prompts.contains(CHANGE_SECTION));
    assert!(
        prompts.contains("helsinki-news"),
        "the model is shown the endpoint"
    );
    let tool = &events[0].payload;
    assert_eq!(tool["status"], "ok", "{tool}");
    assert_eq!(
        tool["output"],
        json!({ "kind": "Endpoint", "name": "helsinki-news", "checked": true })
    );
    let draft = state
        .drafts
        .get("helsinki", "Endpoint", "helsinki-news")
        .await
        .expect("drafts")
        .expect("the edit is the person's draft");
    assert_eq!(draft.manifest["spec"]["slug"], "newsnewsnewsnewsnewsnewsne");
    assert_eq!(
        draft.manifest["spec"]["enabledRepresentations"],
        json!(["ngsi-ld", "geojson", "csv"])
    );
    assert_eq!(
        draft.manifest["metadata"]["labels"]["joinedcontext.com/space"],
        "helsinki"
    );
    let navigate = events
        .iter()
        .find(|e| e.kind == "navigate")
        .expect("the form opens");
    assert_eq!(navigate.payload["route"], "/projects/helsinki/endpoints");
    assert_eq!(
        navigate.payload["draft"],
        json!({ "kind": "Endpoint", "name": "helsinki-news" })
    );
    assert_eq!(navigate.payload["prefill"]["existing"], true);
    assert_eq!(
        navigate.payload["prefill"]["slug"],
        "newsnewsnewsnewsnewsnewsne"
    );
    assert_eq!(
        navigate.payload["prefill"]["enabledRepresentations"],
        json!(["ngsi-ld", "geojson", "csv"])
    );
}

#[tokio::test]
async fn pausing_a_pipeline_opens_its_editor_with_the_patched_manifest_and_proposes_nothing() {
    let (state, events, prompts) = converse_with(
        Some(pipeline_access()),
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: "Pausing the news pipeline.\n\n```json\n{\"tool\":\"change_resource\",\"kind\":\"Pipeline\",\"name\":\"hel-news\",\"patch\":{\"spec\":{\"enabled\":false}}}\n```\n",
            message: "Pause the hel-news pipeline",
            tool: "change_resource",
            seeded: news_pipeline(),
        },
    )
    .await;

    assert!(prompts.contains(CHANGE_SECTION));
    assert!(
        prompts.contains("hel-news"),
        "the model is shown the pipeline"
    );
    assert!(
        !prompts.contains("\"ContextSpace\": ["),
        "a kind outside the profile is not offered"
    );
    let tool = &events[0].payload;
    assert_eq!(tool["status"], "ok", "{tool}");
    let navigate = events
        .iter()
        .find(|e| e.kind == "navigate")
        .expect("the editor opens");
    assert_eq!(
        navigate.payload["route"],
        "/projects/helsinki/pipelines?edit=hel-news"
    );
    assert_eq!(
        navigate.payload["draft"],
        json!({ "kind": "Pipeline", "name": "hel-news" })
    );
    let prefill = &navigate.payload["prefill"];
    assert_eq!(prefill["kind"], "Pipeline");
    assert_eq!(prefill["spec"]["enabled"], false);
    assert_eq!(prefill["spec"]["class"], "resident");
    assert!(prefill.get("status").is_none(), "{prefill}");
    let draft = state
        .drafts
        .get("helsinki", "Pipeline", "hel-news")
        .await
        .expect("drafts")
        .expect("the change is the person's draft");
    assert_eq!(draft.manifest["spec"]["enabled"], false);
    assert!(
        draft
            .verdict
            .as_ref()
            .is_some_and(|verdict| verdict.is_fresh_for(&draft.manifest)),
        "the draft carries the check it passed"
    );
    // The pipeline itself is untouched: only the person's proposal changes it.
    let stored = state
        .mirror
        .get("helsinki", "Pipeline", "hel-news")
        .expect("pipeline");
    assert_eq!(stored.spec["enabled"], true);
    assert!(events.iter().all(|e| e.kind != "change"));
}

#[tokio::test]
async fn a_change_the_check_refuses_goes_back_to_the_model_with_the_findings() {
    let (state, events, prompts) = converse_with(
        Some(pipeline_access()),
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: "```json\n{\"tool\":\"change_resource\",\"kind\":\"Pipeline\",\"name\":\"hel-news\",\"patch\":{\"spec\":{\"class\":\"whenever\"}}}\n```",
            message: "Run hel-news whenever",
            tool: "change_resource",
            seeded: news_pipeline(),
        },
    )
    .await;

    let tool = &events[0].payload;
    assert_eq!(tool["status"], "failed", "{tool}");
    assert!(
        prompts.contains("the platform's check refuses the change"),
        "the findings are sent back to the model"
    );
    assert!(events.iter().all(|e| e.kind != "navigate"));
    assert!(state
        .drafts
        .get("helsinki", "Pipeline", "hel-news")
        .await
        .expect("drafts")
        .is_none());
}

#[tokio::test]
async fn a_change_to_a_name_that_does_not_exist_answers_the_real_ones() {
    let (_, events, prompts) = converse_with(
        Some(pipeline_access()),
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: "```json\n{\"tool\":\"change_resource\",\"kind\":\"Pipeline\",\"name\":\"hel-parking\",\"patch\":{\"spec\":{\"enabled\":false}}}\n```",
            message: "Pause the parking pipeline",
            tool: "change_resource",
            seeded: news_pipeline(),
        },
    )
    .await;

    let tool = &events[0].payload;
    assert_eq!(tool["status"], "failed", "{tool}");
    assert!(
        tool["error"]
            .as_str()
            .is_some_and(|e| e.contains("hel-parking") && e.contains("hel-news")),
        "{tool}"
    );
    assert!(prompts.contains("the project's are hel-news"));
    assert!(events.iter().all(|e| e.kind != "navigate"));
}

#[tokio::test]
async fn a_kind_outside_the_profile_is_refused_with_the_reason() {
    let (_, events, _) = converse_with(
        Some(pipeline_access()),
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: "```json\n{\"tool\":\"change_resource\",\"kind\":\"ContextSpace\",\"name\":\"helsinki\",\"delete\":true}\n```",
            message: "Remove the helsinki space",
            tool: "change_resource",
            seeded: news_pipeline(),
        },
    )
    .await;

    let tool = &events[0].payload;
    assert_eq!(tool["status"], "failed", "{tool}");
    assert!(
        tool["error"]
            .as_str()
            .is_some_and(|e| e.contains("propose on ContextSpace")),
        "{tool}"
    );
    assert!(events.iter().all(|e| e.kind != "navigate"));
}

#[tokio::test]
async fn a_removal_opens_the_typed_confirmation_of_the_resource_and_a_person_without_delete_is_refused(
) {
    let access = json!({
        "operations": ["jc_resource_propose"],
        "kinds": [{ "kind": "ContextSpace", "verbs": ["read", "propose"] }]
    });
    let removal = "Opening its removal.\n\n```json\n{\"tool\":\"change_resource\",\"kind\":\"ContextSpace\",\"name\":\"helsinki\",\"delete\":true}\n```\n";
    let (_, events, _) = converse_with(
        Some(access.clone()),
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: removal,
            message: "Remove the helsinki space",
            tool: "change_resource",
            seeded: bikes_space(),
        },
    )
    .await;
    assert_eq!(events[0].payload["status"], "ok", "{}", events[0].payload);
    let navigate = events
        .iter()
        .find(|e| e.kind == "navigate")
        .expect("the removal dialog opens");
    assert_eq!(
        navigate.payload,
        json!({ "route": "/projects/helsinki/spaces?delete=helsinki" })
    );

    // The reader may start a conversation but holds no delete on context spaces.
    let (_, events, _) = converse_with(
        Some(access),
        person("reader@hel.fi", &[]),
        Conversation {
            answer: removal,
            message: "Remove the helsinki space",
            tool: "change_resource",
            seeded: bikes_space(),
        },
    )
    .await;
    assert_eq!(
        events[0].payload["status"], "failed",
        "{}",
        events[0].payload
    );
    assert!(events.iter().all(|e| e.kind != "navigate"));
}

#[tokio::test]
async fn a_feed_url_with_a_description_is_integrated_as_drafts_the_person_reviews() {
    let access = json!({
        "operations": ["jc_catalog_search", "jc_space_complete"],
        "kinds": [{ "kind": "ContextSpace", "verbs": ["read", "propose"] }]
    });
    let (state, events, prompts) = converse_with(
        Some(access),
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: "I will integrate the weather feed.\n\n```json\n{\"tool\":\"space_complete\",\"space\":\"helsinki-weather\",\"url\":\"https://example.invalid/weather.json\",\"typeName\":\"WeatherObserved\",\"description\":\"Hourly observations of the city's weather stations.\"}\n```\n",
            message: "integrate https://example.invalid/weather.json, hourly weather observations: temperature, wind",
            tool: "space_complete",
            seeded: Vec::new(),
        },
    )
    .await;

    assert!(prompts.contains("## WHEN THE PERSON ASKS TO COMPLETE A CONTEXT SPACE"));
    assert!(
        prompts.contains("typeName"),
        "the model is told the integration call"
    );
    let tool = &events[0].payload;
    assert_eq!(tool["status"], "ok", "{tool}");
    assert_eq!(
        tool["input"],
        json!({
            "space": "helsinki-weather",
            "url": "https://example.invalid/weather.json",
            "typeName": "WeatherObserved",
            "description": "Hourly observations of the city's weather stations."
        }),
        "the operation gets the call's own fields, never `tool` or a proposal"
    );
    assert!(tool["output"]["change"].is_null(), "{tool}");
    let model = state
        .drafts
        .get("helsinki", "DataModel", "helsinki-weather")
        .await
        .expect("drafts")
        .expect("the model is the person's draft");
    assert_eq!(
        model.manifest["spec"]["classes"],
        json!(["WeatherObserved"])
    );
    let navigate = events
        .iter()
        .find(|e| e.kind == "navigate")
        .expect("the drafts open");
    assert_eq!(
        navigate.payload["route"],
        "/projects/helsinki/spaces/complete?space=helsinki-weather"
    );
    // The drafts travel with the navigation, so the page opens ready to propose (AG-73).
    assert_eq!(
        navigate.payload["prefill"]["result"]["space"],
        "helsinki-weather"
    );
    assert!(navigate.payload["prefill"]["result"]["drafts"].is_array());
}

#[tokio::test]
async fn an_edit_endpoint_call_of_the_earlier_prompt_still_names_the_real_endpoints() {
    let endpoint = envelope(
        "Endpoint",
        "helsinki-news",
        "helsinki",
        json!({ "contextSpaceRef": "helsinki", "slug": "newsnewsnewsnewsnewsnewsne", "audience": "public", "enabledRepresentations": ["ngsi-ld"] }),
    );
    let (state, events, _) = converse_with(
        None,
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: "```json\n{\"tool\":\"edit_endpoint\",\"name\":\"helsinki-parking\",\"audience\":\"public\"}\n```",
            message: "Make helsinki-parking public",
            tool: "edit_endpoint",
            seeded: vec![endpoint],
        },
    )
    .await;

    let tool = &events[0].payload;
    assert_eq!(tool["status"], "failed", "{tool}");
    assert!(
        tool["error"]
            .as_str()
            .is_some_and(|e| e.contains("helsinki-news")),
        "{tool}"
    );
    assert!(events.iter().all(|e| e.kind != "navigate"));
    assert!(state
        .drafts
        .get("helsinki", "Endpoint", "helsinki-parking")
        .await
        .expect("drafts")
        .is_none());
}

#[tokio::test]
async fn a_profile_naming_an_operation_the_portal_does_not_register_is_refused_on_admission() {
    let config = config("http://proxy.invalid");
    let app = server::app(AppState::new(config.clone(), None).with_mirror(mirror(None)));
    let mut profile = mirror(None)
        .get(ORG_NAMESPACE, "AgentProfile", "app-builder")
        .expect("the seeded profile");
    profile.spec["access"] = json!({ "operations": ["jc_catalog_search", "jc_launch_rockets"] });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/projects/{ORG_NAMESPACE}/agentprofiles?dryRun=All"
                ))
                .header(
                    header::COOKIE,
                    cookie(&config, person("admin@hel.fi", &["portal-approver"])),
                )
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&profile).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("jc_launch_rockets") && body.contains("MF-40"),
        "{body}"
    );
}

#[tokio::test]
async fn the_access_view_names_both_halves_and_the_one_that_refuses() {
    let config = config("http://proxy.invalid");
    let access = json!({
        "operations": ["jc_catalog_search", "jc_endpoint_propose"],
        "kinds": [{ "kind": "Endpoint", "verbs": ["read", "propose"] }]
    });
    let app = server::app(AppState::new(config.clone(), None).with_mirror(mirror(Some(access))));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/helsinki/assistant/access")
                .header(
                    header::COOKIE,
                    cookie(&config, person("reader@hel.fi", &[])),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    let profile = &body["items"][0];
    assert_eq!(profile["name"], "app-builder");
    assert_eq!(profile["egressHosts"], json!(["registry.npmjs.org"]));
    assert_eq!(profile["access"]["operations"][1], "jc_endpoint_propose");
    let op = |name: &str| {
        profile["operations"]
            .as_array()
            .and_then(|ops| ops.iter().find(|op| op["name"] == name))
            .cloned()
            .expect("listed")
    };
    let search = op("jc_catalog_search");
    assert_eq!(
        (search["profile"].clone(), search["person"].clone()),
        (json!(true), json!(true))
    );
    assert!(search["reason"].is_null());
    let share = op("jc_endpoint_propose");
    assert_eq!(
        (share["profile"].clone(), share["person"].clone()),
        (json!(true), json!(false))
    );
    assert!(
        share["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "{share}"
    );
    let complete = op("jc_space_complete");
    assert_eq!(complete["profile"], false);
    assert!(complete["reason"]
        .as_str()
        .is_some_and(|r| r.contains("does not grant")));
}

const KPI_PIPELINE_SECTION: &str = "## WHEN THE PERSON ASKS TO KEEP AN INDICATOR UPDATED";

const KPI_PIPELINE_ANSWER: &str = "I will recompute the free bikes into transportation-kpi on every change.\n\n```json\n{\"tool\":\"draft_kpi_pipeline\",\"name\":\"free-bikes\",\"title\":\"Free bikes\",\"type\":\"BikeHireDockingStation\",\"attribute\":\"availableBikeNumber\",\"agg\":\"sum\",\"unit\":\"C62\",\"sourceEndpoint\":\"helsinki-all\",\"targetSpace\":\"transportation-kpis\",\"onChange\":true}\n```\n";

/// The data space and its endpoint, which the indicator pipeline reads.
fn bikes_space() -> Vec<ResourceEnvelope> {
    vec![
        envelope(
            "ContextSpace",
            "helsinki",
            "helsinki",
            json!({ "isSandbox": false, "defaultLocale": "en" }),
        ),
        envelope(
            "Endpoint",
            "helsinki-all",
            "helsinki",
            json!({ "contextSpaceRef": "helsinki", "slug": "allallallallallallallallal", "audience": "organization" }),
        ),
    ]
}

#[tokio::test]
async fn an_indicator_kept_updated_is_a_drafted_pipeline_into_a_new_indicator_space() {
    let access = json!({
        "operations": ["jc_catalog_search", "jc_pipeline_propose", "jc_pipeline_test"],
        "kinds": [{ "kind": "Pipeline", "verbs": ["read", "propose"] }]
    });
    let (state, events, prompts) = converse_with(
        Some(access),
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: KPI_PIPELINE_ANSWER,
            message: "Keep the number of free bikes updated in transportation-kpis on every change",
            tool: "draft_kpi_pipeline",
            seeded: bikes_space(),
        },
    )
    .await;

    assert!(prompts.contains(KPI_PIPELINE_SECTION));
    let tool = &events[0].payload;
    assert_eq!(tool["status"], "ok", "{tool}");
    let output = &tool["output"];
    assert_eq!(output["targetSpace"], "transportation-kpi");
    assert_eq!(output["trigger"], "on every change of availableBikeNumber");
    assert_eq!(
        output["formula"],
        "sum(availableBikeNumber) over BikeHireDockingStation"
    );
    // No runner in the test: the verdict says so instead of pretending a pass.
    assert_eq!(output["verdict"]["ok"], false, "{output}");
    assert!(output["verdict"]["untested"].is_string(), "{output}");
    let kinds: Vec<&str> = output["drafts"]
        .as_array()
        .expect("drafts")
        .iter()
        .filter_map(|d| d["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["ContextSpace", "Endpoint", "Policy", "Policy"]);
    assert_eq!(output["runnerAudience"], output["targetSlug"]);

    let pipeline = state
        .drafts
        .get("helsinki", "Pipeline", "free-bikes")
        .await
        .expect("drafts")
        .expect("the pipeline is the person's draft");
    let spec = &pipeline.manifest["spec"];
    assert_eq!(
        spec["source"]["trigger"]["subscription"]["watchedAttributes"],
        json!(["availableBikeNumber"])
    );
    assert_eq!(
        spec["targetEndpoint"],
        "urn:ngsi-ld:Endpoint:hel.fi:transportation-kpi:transportation-kpi"
    );
    assert_eq!(pipeline.touched_by, "admin");
    for (kind, name) in [
        ("ContextSpace", "transportation-kpi"),
        ("Endpoint", "transportation-kpi"),
        ("Policy", "transportation-kpi-pipelines-write"),
        ("Policy", "transportation-kpi-read"),
    ] {
        assert!(
            state
                .drafts
                .get("helsinki", kind, name)
                .await
                .expect("drafts")
                .is_some(),
            "{kind}/{name} is drafted"
        );
    }

    let navigate = events
        .iter()
        .find(|e| e.kind == "navigate")
        .expect("the form opens");
    assert_eq!(navigate.payload["route"], "/projects/helsinki/pipelines");
    assert_eq!(
        navigate.payload["draft"],
        json!({ "kind": "Pipeline", "name": "free-bikes" })
    );
    assert_eq!(
        navigate.payload["prefill"]["source"]["endpointRef"],
        "helsinki-all"
    );
    assert!(state
        .drafts
        .get("helsinki", "Pipeline", "free-bikes")
        .await
        .expect("drafts")
        .is_some());
}

#[tokio::test]
async fn an_indicator_pipeline_outside_the_profile_is_neither_offered_nor_drafted() {
    let access = json!({ "operations": ["jc_catalog_search"], "kinds": [] });
    let (state, events, prompts) = converse_with(
        Some(access),
        person("admin@hel.fi", &["portal-approver"]),
        Conversation {
            answer: KPI_PIPELINE_ANSWER,
            message: "Keep the number of free bikes updated on every change",
            tool: "draft_kpi_pipeline",
            seeded: bikes_space(),
        },
    )
    .await;

    assert!(!prompts.contains(KPI_PIPELINE_SECTION));
    let tool = &events[0].payload;
    assert_eq!(tool["status"], "failed", "{tool}");
    assert!(tool["error"]
        .as_str()
        .is_some_and(|e| e.contains("jc_pipeline_propose")));
    assert!(state
        .drafts
        .get("helsinki", "Pipeline", "free-bikes")
        .await
        .expect("drafts")
        .is_none());
    assert!(state
        .drafts
        .get("helsinki", "ContextSpace", "transportation-kpi")
        .await
        .expect("drafts")
        .is_none());
}

/// Starts a conversation on `endpoints` whose model answers `answers` in order, with the proxy
/// serving the endpoint's MCP façade; returns the run's events and every model request body.
async fn ask_the_data(endpoints: Value, answers: &[&str]) -> (Vec<AgentRunEvent>, Vec<Value>) {
    let proxy = MockServer::start().await;
    for answer in answers {
        Mock::given(method("POST"))
            .and(path("/v1/llm/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "chatcmpl-1", "object": "chat.completion",
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": answer }, "finish_reason": "stop" }],
                "usage": { "total_tokens": 900 }
            })))
            .up_to_n_times(1)
            .mount(&proxy)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/v1/data/mcp"))
        .and(wiremock::matchers::body_string_contains("tools/list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": { "tools": [
                { "name": "query_entities", "description": "Query the entities", "annotations": { "readOnlyHint": true },
                  "inputSchema": { "type": "object", "properties": { "type": { "type": "string" }, "q": { "type": "string" } }, "required": ["type"] } },
                { "name": "upsert_entity", "description": "Write", "annotations": { "readOnlyHint": false }, "inputSchema": { "type": "object" } }
            ] }
        })))
        .mount(&proxy)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/data/mcp"))
        .and(wiremock::matchers::body_string_contains("tools/call"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 1, "result": {
                "content": [{ "type": "text", "text": "[{\"id\":\"urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:kaivopuisto\",\"name\":\"Kaivopuisto\",\"availableBikeNumber\":0}]" }]
            }
        })))
        .mount(&proxy)
        .await;
    let config = config(&proxy.uri());
    let mirror = mirror(None);
    for envelope in bikes_space() {
        mirror.upsert(envelope);
    }
    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/helsinki/assistant/conversations")
                .header(header::COOKIE, cookie(&config, person("admin@hel.fi", &["portal-approver"])))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "message": "Which stations have no bikes?", "endpointNames": endpoints }).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let created: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let id = created["id"].as_str().expect("run id").to_owned();
    let mut events = Vec::new();
    for _ in 0..200 {
        events = state.agents.events_since(&id, 0).await.expect("events");
        let answered = events.iter().filter(|e| e.kind == "thought").any(|e| {
            e.payload["text"]
                .as_str()
                .is_some_and(|t| t.contains("Kaivopuisto") || t.contains("calls one message"))
        });
        if answered {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let bodies = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/v1/llm/chat/completions")
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect();
    (events, bodies)
}

const QUERY_ANSWER: &str = "Let me read the stations.\n```json\n{\"tool\":\"query_endpoint\",\"endpoint\":\"helsinki-all\",\"name\":\"query_entities\",\"arguments\":{\"type\":\"BikeHireDockingStation\",\"q\":\"availableBikeNumber==0\"}}\n```";
const WRITE_ANSWER: &str = "```json\n{\"tool\":\"query_endpoint\",\"endpoint\":\"helsinki-all\",\"name\":\"upsert_entity\",\"arguments\":{}}\n```";
const DATA_PROSE: &str = "One station has no bikes right now: Kaivopuisto.";

#[tokio::test]
async fn a_question_about_the_data_is_answered_from_the_endpoints_the_person_chose() {
    let (events, bodies) = ask_the_data(
        json!(["helsinki-all"]),
        &[WRITE_ANSWER, QUERY_ANSWER, DATA_PROSE],
    )
    .await;

    let steps: Vec<&Value> = events
        .iter()
        .filter(|e| e.kind == "tool" && e.payload["tool"] == "query_endpoint")
        .map(|e| &e.payload)
        .collect();
    assert_eq!(steps.len(), 2, "{events:?}");
    // A write tool is never called: the model is told, and corrects itself.
    assert_eq!(steps[0]["status"], "failed");
    assert!(steps[0]["error"]
        .as_str()
        .is_some_and(|e| e.contains("not a read tool")));
    assert_eq!(steps[1]["status"], "ok", "{}", steps[1]);
    assert_eq!(
        steps[1]["input"]["arguments"]["q"],
        "availableBikeNumber==0"
    );

    assert_eq!(bodies.len(), 3);
    let first = bodies[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(first.contains("## WORKING WITH THE DATA"));
    assert!(first.contains("query_entities") && !first.contains("upsert_entity"));
    let last = bodies[2]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(last.contains("## WHAT YOUR CALLS ANSWERED"));
    assert!(last.contains("Kaivopuisto"));
    assert!(events
        .iter()
        .any(|e| e.kind == "thought" && e.payload["text"] == DATA_PROSE));
}

/// Two calls in one answer, on an endpoint the conversation did not read yet.
const TWO_CALLS: &str = "Let me look at the stations and their types.\n```json\n{\"tool\":\"query_endpoint\",\"endpoint\":\"helsinki-all\",\"name\":\"query_entities\",\"arguments\":{\"type\":\"BikeHireDockingStation\",\"q\":\"availableBikeNumber==0\"}}\n```\n```json\n{\"tool\":\"query_endpoint\",\"endpoint\":\"helsinki-all\",\"name\":\"query_entities\",\"arguments\":{\"type\":\"BikeHireDockingStation\"}}\n```";

#[tokio::test]
async fn a_conversation_without_endpoints_opens_the_one_the_model_names_and_runs_its_calls_at_once()
{
    let (events, bodies) = ask_the_data(json!([]), &[TWO_CALLS, DATA_PROSE]).await;

    // The prompt offers the project's endpoints the person may open.
    let first = bodies[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(first.contains("## WORKING WITH THE DATA"));
    assert!(first.contains("\"endpoint\": \"helsinki-all\""), "{first}");

    // The endpoint is added to the conversation, as the data bar shows it, before the calls.
    let opened = events
        .iter()
        .position(|e| e.kind == "endpoints")
        .expect("an endpoints event");
    assert_eq!(events[opened].payload["names"], json!(["helsinki-all"]));
    let steps: Vec<(usize, &Value)> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == "tool" && e.payload["tool"] == "query_endpoint")
        .map(|(i, e)| (i, &e.payload))
        .collect();
    assert_eq!(steps.len(), 2, "{events:?}");
    assert!(steps
        .iter()
        .all(|(i, step)| *i > opened && step["status"] == "ok"));

    // Both results go back together in one more model call, which answers.
    assert_eq!(bodies.len(), 2);
    let last = bodies[1]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(last.contains("### Call 1") && last.contains("### Call 2"));
    assert!(last.contains("Kaivopuisto"));
    assert!(events
        .iter()
        .any(|e| e.kind == "thought" && e.payload["text"] == DATA_PROSE));
}
