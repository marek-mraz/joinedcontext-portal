//! Tests for the Portal operation registry and its REST adapter (T-0636, AG-59, CC-48, PF-50).
//!
//! Asserts that:
//! 1. Every registered operation answers on `POST /api/v1/projects/{project}/ops/{name}`.
//! 2. `GET /api/v1/projects/{project}/ops` lists only operations permitted for the caller.
//! 3. An unknown field in input yields 422 Unprocessable Entity with error details.
//! 4. A principal without permissions is refused with 403 Forbidden.
//! 5. `jc_catalog_search` produces identical search results to the assistant catalog route.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const TEST_CSRF_TOKEN: &str = "test-csrf-token-ops-123";

fn session_cookie(
    config: &Config,
    username: &str,
    email: Option<&str>,
    roles: Vec<&str>,
    groups: Vec<&str>,
) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            subject: format!("sub-{username}"),
            username: username.to_string(),
            email: email.map(str::to_string),
            name: Some(username.to_string()),
            roles: roles.into_iter().map(str::to_string).collect(),
            groups: groups.into_iter().map(str::to_string).collect(),
        },
        expires_at: now + 3600,
        issued_at: now,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        let pair = raw.split(';').next().unwrap_or_default();
        parts.push(pair.to_string());
    }
    parts.push(format!("{CSRF_COOKIE}={TEST_CSRF_TOKEN}"));
    parts.join("; ")
}

#[tokio::test]
async fn every_registered_name_answers_on_post_ops() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    for op in ops::registry() {
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/projects/ovzdusie/ops/{}", op.name))
            .header(header::COOKIE, &steward_cookie)
            .header(CSRF_HEADER, TEST_CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b"{}".to_vec()))
            .expect("request");

        let resp = app.clone().oneshot(req).await.expect("response");
        let status = resp.status();

        // An operation may succeed (200/202) or reject input/conditions (400/403/409/422/503),
        // but must NEVER be 404 (unregistered route) or 500 (unhandled panic).
        assert_ne!(
            status,
            StatusCode::NOT_FOUND,
            "operation '{}' was not found on POST /ops/{{name}}",
            op.name
        );
        assert_ne!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "operation '{}' failed with 500 Internal Server Error",
            op.name
        );
    }
}

#[tokio::test]
async fn get_ops_differs_between_steward_and_viewer() {
    let config = Config::for_tests();
    let mirror = std::sync::Arc::new(Mirror::new());

    // Role that only allows reading/viewing, no propose/approve/delete verbs
    let viewer_role = ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Role".to_string(),
        metadata: ObjectMeta::new("read-only-role", "org"),
        spec: json!({
            "rules": [
                { "kinds": ["Endpoint", "ContextSpace"], "verbs": [] }
            ]
        }),
        status: None,
    };
    let viewer_binding = ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "RoleBinding".to_string(),
        metadata: ObjectMeta::new("viewer-binding", "org"),
        spec: json!({
            "subjects": [{ "group": "city-viewers" }],
            "role": "read-only-role",
            "scope": { "project": "ovzdusie" }
        }),
        status: None,
    };
    mirror.upsert(viewer_role);
    mirror.upsert(viewer_binding);

    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let app = server::app(state);

    // 1. Steward has bootstrap group
    let steward_cookie = session_cookie(
        &config,
        "demo.steward",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );
    let resp_steward = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/projects/ovzdusie/ops")
                .header(header::COOKIE, &steward_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_steward.status(), StatusCode::OK);
    let bytes_steward = resp_steward.into_body().collect().await.unwrap().to_bytes();
    let steward_ops: Vec<Value> = serde_json::from_slice(&bytes_steward).unwrap();
    let steward_names: Vec<&str> = steward_ops
        .iter()
        .filter_map(|o| o["name"].as_str())
        .collect();

    assert!(steward_names.contains(&"jc_datasource_propose"));
    assert!(steward_names.contains(&"jc_catalog_search"));
    assert!(steward_names.contains(&"jc_change_approve"));

    // 2. Viewer has only read-only grant
    let viewer_cookie = session_cookie(
        &config,
        "demo.viewer",
        Some("viewer@banskabystrica.sk"),
        vec![],
        vec!["city-viewers"],
    );
    let resp_viewer = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/projects/ovzdusie/ops")
                .header(header::COOKIE, &viewer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_viewer.status(), StatusCode::OK);
    let bytes_viewer = resp_viewer.into_body().collect().await.unwrap().to_bytes();
    let viewer_ops: Vec<Value> = serde_json::from_slice(&bytes_viewer).unwrap();
    let viewer_names: Vec<&str> = viewer_ops
        .iter()
        .filter_map(|o| o["name"].as_str())
        .collect();

    assert!(
        viewer_names.contains(&"jc_catalog_search"),
        "viewer can run read-only operations"
    );
    assert!(
        !viewer_names.contains(&"jc_datasource_propose"),
        "viewer must not see mutating propose operations"
    );
    assert!(
        !viewer_names.contains(&"jc_change_approve"),
        "viewer must not see approve operations"
    );
}

#[tokio::test]
async fn unknown_field_in_input_returns_422() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let payload = json!({
        "q": "bikes",
        "extraneousField": "should-trigger-422"
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_catalog_search")
                .header(header::COOKIE, steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let error_body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(error_body["error"], "invalid_input");
    assert!(
        error_body["message"]
            .as_str()
            .unwrap()
            .contains("extraneousField")
            || error_body["path"]
                .as_str()
                .unwrap()
                .contains("extraneousField")
    );
}

#[tokio::test]
async fn principal_without_binding_returns_403() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let unprivileged_cookie = session_cookie(
        &config,
        "unprivileged.user",
        Some("unprivileged@example.com"),
        vec![],
        vec![],
    );

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_catalog_search")
                .header(header::COOKIE, unprivileged_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b"{\"q\":\"bikes\"}".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn jc_catalog_search_matches_assistant_route() {
    let config = Config::for_tests();
    let mirror = std::sync::Arc::new(Mirror::new());

    // Populate mirror with a context space and an endpoint
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta::new("mobility", "ovzdusie"),
        spec: json!({ "isSandbox": false }),
        status: None,
    });
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "Endpoint".to_string(),
        metadata: ObjectMeta::new("public-air", "ovzdusie"),
        spec: json!({
            "contextSpaceRef": "mobility",
            "slug": "publicair00000000000000000000",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }),
        status: None,
    });

    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let app = server::app(state);

    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    // 1. Call assistant catalog route
    let resp_assistant = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/projects/ovzdusie/assistant/catalog?q=air")
                .header(header::COOKIE, &steward_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_assistant.status(), StatusCode::OK);
    let bytes_asst = resp_assistant
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let asst_json: Value = serde_json::from_slice(&bytes_asst).unwrap();

    // 2. Call ops registry generic route
    let resp_ops = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_catalog_search")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b"{\"q\":\"air\"}".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_ops.status(), StatusCode::OK);
    let bytes_ops = resp_ops.into_body().collect().await.unwrap().to_bytes();
    let ops_json: Value = serde_json::from_slice(&bytes_ops).unwrap();

    // Both should return catalog item "public-air"
    let asst_items = asst_json["items"].as_array().expect("items array");
    let ops_items = ops_json["items"].as_array().expect("items array");

    assert_eq!(asst_items.len(), ops_items.len());
    assert_eq!(asst_items[0]["name"], ops_items[0]["name"]);
    assert_eq!(ops_items[0]["name"], "public-air");
}

/// AG-60, CC-47, T-0838: what a tool publishes as its output is what the tool answers.
///
/// A schema pointing at `#/components/schemas/…` describes nothing to an MCP client: the
/// pointer has no document to resolve against, so every published schema is self-contained.
#[test]
fn no_published_schema_points_at_a_document_the_caller_never_has() {
    fn refs(value: &Value, path: &str, found: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    if key == "$ref" {
                        found.push(format!("{path}: {inner}"));
                    }
                    refs(inner, &format!("{path}/{key}"), found);
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    refs(item, &format!("{path}/{index}"), found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for op in ops::registry() {
        refs(&(op.input)(), &format!("{}.input", op.name), &mut found);
        refs(&(op.output)(), &format!("{}.output", op.name), &mut found);
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The propose and delete operations answer the wrapper, not the bare `Change`: what the
/// schema names as required is what the answer carries (T-0838).
#[test]
fn the_change_operations_publish_the_wrapper_they_answer() {
    for name in [
        "jc_resource_propose",
        "jc_resource_delete",
        "jc_datasource_propose",
        "jc_pipeline_propose",
        "jc_space_propose",
        "jc_model_propose",
        "jc_change_reject",
    ] {
        let op = ops::find(name).expect("registered");
        let schema = (op.output)();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            required,
            vec!["changeId", "lane", "change"],
            "{name} answers ProposeOutcome::Change"
        );
        assert!(schema["properties"]["change"]["properties"]["status"].is_object());
    }

    // `jc_endpoint_propose` renders a proposal from parameters and answers a Change from a
    // manifest or a draft: both shapes are published, neither is hidden.
    let endpoint = (ops::find("jc_endpoint_propose").expect("registered").output)();
    let shapes = endpoint["oneOf"].as_array().expect("both shapes");
    assert_eq!(shapes[0]["required"], json!(["changeId", "lane", "change"]));
    assert!(shapes[1]["properties"]["endpoint"].is_object());

    // The approval answers the `Change` itself, and the list its envelope.
    let approve = (ops::find("jc_change_approve").expect("registered").output)();
    assert_eq!(approve["properties"]["kind"]["enum"], json!(["Change"]));
    let list = (ops::find("jc_change_list").expect("registered").output)();
    assert_eq!(list["properties"]["kind"]["enum"], json!(["ChangeList"]));
    assert_eq!(list["properties"]["items"]["type"], json!("array"));
}

/// AG-59, CC-48, T-0840: what the Portal serves, the registry serves. These four reads had a
/// route and no operation, so an MCP client could not see a pipeline's counters, what happened
/// in the project, the federation, or a model's LinkML.
#[tokio::test]
async fn the_reads_that_had_only_a_route_answer_through_the_registry() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta::new("ovzdusie", "ovzdusie"),
        spec: json!({ "isSandbox": false }),
        status: None,
    });
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta::new("air", "ovzdusie"),
        spec: json!({
            "version": "1.0.0",
            "lifecycle": "draft",
            "contextSpaceRef": "ovzdusie",
            "linkml": "id: https://example.org/air\nname: air\nclasses:\n  AirQualityObserved:\n    slots: [pm10]\n",
            "classes": ["AirQualityObserved"]
        }),
        status: None,
    });
    let state = AppState::new(config, None).with_mirror(mirror);
    let caller = ops::Caller {
        identity: joinedcontext_portal::auth::session::Identity {
            subject: "sub-steward".into(),
            username: "steward".into(),
            email: Some("steward@banskabystrica.sk".into()),
            name: None,
            roles: vec!["portal-approver".into()],
            groups: vec!["platform-admins".into()],
        },
        via: ops::Via::Mcp,
    };

    let graph = ops::call(
        ops::find("jc_federation_graph").expect("registered"),
        &caller,
        &state,
        "ovzdusie",
        json!({}),
    )
    .await
    .expect("the federation");
    assert!(graph["nodes"].is_array(), "{graph}");

    let activity = ops::call(
        ops::find("jc_activity_list").expect("registered"),
        &caller,
        &state,
        "ovzdusie",
        json!({ "severity": "warning", "limit": 10 }),
    )
    .await
    .expect("what happened");
    assert!(activity["items"].is_array(), "{activity}");

    let source = ops::call(
        ops::find("jc_model_source_get").expect("registered"),
        &caller,
        &state,
        "ovzdusie",
        json!({ "name": "air" }),
    )
    .await
    .expect("the model's LinkML");
    assert!(
        source["source"]
            .as_str()
            .unwrap_or_default()
            .contains("AirQualityObserved"),
        "{source}"
    );

    // A pipeline that is not there is not disclosed as existing elsewhere (R20).
    let missing = ops::call(
        ops::find("jc_pipeline_metrics").expect("registered"),
        &caller,
        &state,
        "ovzdusie",
        json!({ "name": "no-such-pipeline" }),
    )
    .await
    .expect_err("no such pipeline");
    assert!(format!("{missing:?}").contains("not found"), "{missing:?}");
}
