//! Tests for `jc_space_complete` operation (T-0642).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const TEST_CSRF_TOKEN: &str = "test-csrf-token-space-complete-123";

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
async fn space_complete_rejects_both_or_neither_files_and_url() {
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

    // Neither
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_space_complete")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b"{}".to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // Both
    let both_payload = json!({
        "url": "https://example.com/feed.json",
        "files": [{ "name": "test.csv", "content": "a,b\n1,2" }]
    });
    let resp_both = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_space_complete")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&both_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_both.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn space_complete_viewer_without_propose_is_forbidden() {
    let config = Config::for_tests();
    let mirror = std::sync::Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let app = server::app(state);

    let unpriv_cookie = session_cookie(
        &config,
        "viewer.user",
        Some("viewer@example.com"),
        vec![],
        vec![],
    );

    let payload = json!({
        "url": "https://example.com/feed.json"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_space_complete")
                .header(header::COOKIE, unpriv_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn space_complete_readme_url_candidate_never_fetched() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/unfetched"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&mock_server)
        .await;

    let candidate_url = format!("{}/unfetched", mock_server.uri());

    let mut config = Config::for_tests();
    let model_tools_mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/infer-schema"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "linkml": "id: https://example.com/bikes\nclasses:\n  BikeStation:\n    slots: [name]\n"
        })))
        .mount(&model_tools_mock)
        .await;
    config.model_tools_url = Some(model_tools_mock.uri());

    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let files_payload = json!({
        "space": "bikes",
        "files": [
            { "name": "bike-stations.csv", "content": "stationId,name\n001,Main" },
            { "name": "README.md", "content": format!("Data comes from {}", candidate_url) }
        ]
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_space_complete")
                .header(header::COOKIE, steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&files_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val: Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(val["space"], "bikes");
    let drafts = val["drafts"].as_array().expect("drafts array");
    let ds_draft = drafts
        .iter()
        .find(|d| d["kind"] == "DataSource")
        .expect("DataSource drafted");
    assert_eq!(ds_draft["manifest"]["spec"]["http"]["url"], candidate_url);
    // No Endpoint serves the new space, so one is drafted (organization-wide, EP-02 slug) and
    // the pipeline writes through it: a Pipeline without a target does not even parse.
    let ep_draft = drafts
        .iter()
        .find(|d| d["kind"] == "Endpoint")
        .expect("Endpoint drafted");
    assert_eq!(ep_draft["inferred"], true);
    assert_eq!(ep_draft["manifest"]["spec"]["audience"], "organization");
    assert_eq!(ep_draft["manifest"]["spec"]["contextSpaceRef"], "bikes");
    assert!(ep_draft["manifest"]["spec"]["slug"].as_str().unwrap().len() >= 26);
    let pl_draft = drafts
        .iter()
        .find(|d| d["kind"] == "Pipeline")
        .expect("Pipeline drafted");
    let target = pl_draft["manifest"]["spec"]["targetEndpoint"]
        .as_str()
        .unwrap();
    assert!(target.ends_with(":bikes:bikes-all"), "{target}");
    assert_eq!(val["lane"], "yellow");
    // wiremock assertion verifies 0 requests were received by mock_server
}

#[tokio::test]
async fn space_complete_env_file_creates_secret_ref_without_leaking_password() {
    let mut config = Config::for_tests();
    let model_tools_mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/infer-schema"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "linkml": "id: https://example.com/sensors\nclasses:\n  Sensor:\n    slots: [id]\n"
        })))
        .mount(&model_tools_mock)
        .await;
    config.model_tools_url = Some(model_tools_mock.uri());

    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let env_content = "MQTT_HOST=tcp://broker.local:1883\nMQTT_TOPIC=city/sensors\nMQTT_PASSWORD=supersecret_pass123\n";
    let files_payload = json!({
        "space": "sensors",
        "files": [
            { "name": ".env", "content": env_content },
            { "name": "sensors.csv", "content": "id,val\n1,2\n" }
        ]
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_space_complete")
                .header(header::COOKIE, steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&files_payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val_str = String::from_utf8_lossy(&body_bytes);
    assert!(!val_str.contains("supersecret_pass123"));
    let val: Value = serde_json::from_slice(&body_bytes).unwrap();
    let drafts = val["drafts"].as_array().expect("drafts array");
    let ds_draft = drafts
        .iter()
        .find(|d| d["kind"] == "DataSource")
        .expect("DataSource drafted");
    assert_eq!(
        ds_draft["manifest"]["spec"]["mqtt"]["password"]["secretRef"]["key"],
        "MQTT_PASSWORD"
    );
}

#[tokio::test]
async fn space_complete_all_manifests_provided_yields_found_and_no_inferred_drafts() {
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

    let manifests_yaml = format!(
        r#"apiVersion: {API_VERSION}
kind: ContextSpace
metadata:
  name: test-space
spec:
  isSandbox: false
---
apiVersion: {API_VERSION}
kind: DataModel
metadata:
  name: test-space
spec:
  contextSpaceRef: test-space
  linkml: ./test.linkml.yaml
---
apiVersion: {API_VERSION}
kind: DataSource
metadata:
  name: test-source
spec:
  type: http
  http:
    url: https://example.com/data.json
---
apiVersion: {API_VERSION}
kind: Pipeline
metadata:
  name: test-pipeline
spec:
  class: auto
  source:
    dataSourceRef: test-source
"#
    );

    let payload = json!({
        "space": "test-space",
        "files": [
            { "name": "manifests.yaml", "content": manifests_yaml }
        ]
    });

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/ops/jc_space_complete")
                .header(header::COOKIE, steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val: Value = serde_json::from_slice(&body_bytes).unwrap();
    let found = val["found"].as_array().unwrap();
    assert_eq!(found.len(), 4);
    let drafts = val["drafts"].as_array().unwrap();
    for d in drafts {
        assert_eq!(d["inferred"], false);
    }
}

/// One `jc_space_complete` call as a steward, and its status and body.
async fn complete_as_steward(payload: Value) -> (StatusCode, Value) {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);
    let cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@hel.fi"),
        vec!["portal-approver"],
        vec![],
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/helsinki/ops/jc_space_complete")
                .header(header::COOKIE, cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn draft<'a>(body: &'a Value, kind: &str) -> &'a Value {
    body["drafts"]
        .as_array()
        .and_then(|drafts| drafts.iter().find(|d| d["kind"] == kind))
        .unwrap_or_else(|| panic!("a {kind} draft in {body}"))
}

#[tokio::test]
async fn a_feed_url_with_a_type_and_a_description_names_the_class_and_describes_the_drafts() {
    let (status, body) = complete_as_steward(json!({
        "space": "helsinki-weather",
        "url": "https://example.invalid/weather.json",
        "typeName": "WeatherObserved",
        "description": "Hourly observations of the city's weather stations: temperature in °C, wind in m/s."
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["space"], "helsinki-weather");
    let model = draft(&body, "DataModel");
    assert_eq!(
        model["manifest"]["spec"]["classes"],
        json!(["WeatherObserved"])
    );
    let pipeline = draft(&body, "Pipeline");
    assert_eq!(
        pipeline["manifest"]["spec"]["output"]["type"],
        "WeatherObserved"
    );
    let mapping = pipeline["manifest"]["spec"]["compute"]["bloblang"]
        .as_str()
        .unwrap_or_default();
    assert!(
        mapping.contains("urn:ngsi-ld:WeatherObserved:"),
        "{mapping}"
    );
    let source = draft(&body, "DataSource");
    assert_eq!(
        source["manifest"]["spec"]["http"]["url"],
        "https://example.invalid/weather.json"
    );
    for kind in ["DataModel", "ContextSpace", "DataSource"] {
        assert!(
            draft(&body, kind)["manifest"]["metadata"]["description"]["en"]
                .as_str()
                .is_some_and(|d| d.starts_with("Hourly observations")),
            "{kind} carries the description: {body}"
        );
    }
    assert!(
        body["change"].is_null(),
        "nothing is proposed without the person"
    );
}

#[tokio::test]
async fn a_feed_url_without_a_type_names_the_class_after_the_space_never_entity() {
    let (status, body) = complete_as_steward(json!({
        "space": "helsinki-bikes",
        "url": "https://example.invalid/stations"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        draft(&body, "DataModel")["manifest"]["spec"]["classes"],
        json!(["HelsinkiBike"])
    );
    assert_eq!(
        draft(&body, "Pipeline")["manifest"]["spec"]["output"]["type"],
        "HelsinkiBike"
    );
    assert!(!body.to_string().contains("urn:ngsi-ld:Entity:"), "{body}");
    assert!(draft(&body, "DataModel")["manifest"]["metadata"]["description"].is_null());
}

#[tokio::test]
async fn a_type_name_that_is_not_pascal_case_or_a_description_too_long_is_refused() {
    for payload in [
        json!({ "url": "https://example.invalid/feed.json", "typeName": "weather observed" }),
        json!({ "url": "https://example.invalid/feed.json", "typeName": "urn:ngsi-ld:Entity" }),
        json!({ "url": "https://example.invalid/feed.json", "description": "x".repeat(4001) }),
    ] {
        let (status, body) = complete_as_steward(payload.clone()).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{payload} → {body}"
        );
    }
}
