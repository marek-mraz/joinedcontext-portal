//! Tests for shared drafts, the draft store, and the draft REST API (AG-61, UI-47).

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops::drafts::{DraftError, DraftHub, DraftStore};
use joinedcontext_portal::ops::verdict::{digest_of, Finding, Level, Verdict};
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const TEST_CSRF_TOKEN: &str = "test-csrf-token-drafts-123";

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
async fn memory_store_put_get_version_bump_drop_list() {
    let store = DraftStore::new(None);
    let m1 = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "ds1" },
        "spec": { "type": "http" }
    });

    let d1 = store
        .put(
            "p1",
            "DataSource",
            "ds1",
            m1.clone(),
            None,
            "steward",
            "person",
        )
        .await
        .expect("initial put succeeds");
    assert_eq!(d1.version, 1);
    assert_eq!(d1.touched_by, "steward");
    assert_eq!(d1.touched_kind, "person");

    let got = store
        .get("p1", "DataSource", "ds1")
        .await
        .expect("get succeeds")
        .expect("draft exists");
    assert_eq!(got.version, 1);
    assert_eq!(got.name, "ds1");

    let m2 = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "ds1" },
        "spec": { "type": "http", "http": { "url": "https://example.com" } }
    });
    let d2 = store
        .put("p1", "DataSource", "ds1", m2, Some(1), "steward", "person")
        .await
        .expect("version bump put succeeds");
    assert_eq!(d2.version, 2);

    let err = store
        .put("p1", "DataSource", "ds1", m1, Some(1), "steward", "person")
        .await
        .expect_err("stale expected_version must fail with Conflict");
    match err {
        DraftError::Conflict { current, .. } => assert_eq!(current, 2),
        other => panic!("expected DraftError::Conflict, got {other:?}"),
    }

    let list = store.list("p1").await.expect("list drafts");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "ds1");

    let dropped = store
        .drop("p1", "DataSource", "ds1")
        .await
        .expect("drop draft");
    assert!(dropped);
    assert!(store
        .get("p1", "DataSource", "ds1")
        .await
        .expect("get")
        .is_none());
}

#[tokio::test]
async fn literal_secret_refused_at_put() {
    let store = DraftStore::new(None);
    let secret_manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "ds-secret" },
        "spec": {
            "password": "supersecretpassword"
        }
    });

    let err = store
        .put(
            "p1",
            "DataSource",
            "ds-secret",
            secret_manifest,
            None,
            "steward",
            "person",
        )
        .await
        .expect_err("literal secret must be refused");
    match err {
        DraftError::Secret(field) => assert_eq!(field, "password"),
        other => panic!("expected DraftError::Secret, got {other:?}"),
    }

    let safe_manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "ds-safe" },
        "spec": {
            "password": { "secretRef": { "name": "db-secret", "key": "password" } }
        }
    });
    assert!(store
        .put(
            "p1",
            "DataSource",
            "ds-safe",
            safe_manifest,
            None,
            "steward",
            "person"
        )
        .await
        .is_ok());
}

#[test]
fn digest_of_is_order_independent() {
    let v1 = json!({
        "name": "foo",
        "spec": { "url": "https://example.com", "timeout": 30 }
    });
    let v2 = json!({
        "spec": { "timeout": 30, "url": "https://example.com" },
        "name": "foo"
    });
    assert_eq!(digest_of(&v1), digest_of(&v2));

    let v3 = json!({
        "name": "foo",
        "spec": { "url": "https://example.org", "timeout": 30 }
    });
    assert_ne!(digest_of(&v1), digest_of(&v3));
}

#[test]
fn is_fresh_for_becomes_false_after_manifest_changes() {
    let m1 = json!({ "name": "test", "spec": { "active": true } });
    let verdict = Verdict::green(&m1, None);
    assert!(verdict.is_fresh_for(&m1));

    let m2 = json!({ "name": "test", "spec": { "active": false } });
    assert!(!verdict.is_fresh_for(&m2));

    let red_verdict = Verdict::red(
        &m1,
        vec![Finding {
            level: Level::Error,
            path: "/spec/active".into(),
            message: "validation failed".into(),
        }],
        None,
    );
    assert!(!red_verdict.is_fresh_for(&m1));
}

#[tokio::test]
async fn draft_event_broadcast_on_put_and_drop() {
    let hub = DraftHub::new();
    let store = DraftStore::new(None).with_hub(hub.clone());
    let mut rx = hub.subscribe("p1").await;

    let m = json!({ "spec": { "active": true } });
    store
        .put("p1", "DataSource", "ds-evt", m, None, "steward", "person")
        .await
        .unwrap();

    let evt = rx.recv().await.expect("received put event");
    assert_eq!(evt.project, "p1");
    assert_eq!(evt.kind, "DataSource");
    assert_eq!(evt.name, "ds-evt");
    assert_eq!(evt.event, "put");
    assert_eq!(evt.version, 1);

    store.drop("p1", "DataSource", "ds-evt").await.unwrap();
    let evt_drop = rx.recv().await.expect("received drop event");
    assert_eq!(evt_drop.event, "drop");
    assert_eq!(evt_drop.name, "ds-evt");
}

#[tokio::test]
async fn rest_api_draft_crud_roundtrip() {
    let config = Config::for_tests();
    let mirror = Arc::new(Mirror::new());
    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let app = server::app(state);

    let steward_cookie = session_cookie(
        &config,
        "steward.user",
        Some("steward@banskabystrica.sk"),
        vec!["portal-approver"],
        vec![],
    );

    let put_body = json!({
        "manifest": {
            "apiVersion": API_VERSION,
            "kind": "DataSource",
            "metadata": { "name": "feed-1" },
            "spec": { "type": "http" }
        }
    });

    let resp_put = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/drafts/DataSource/feed-1")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&put_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_put.status(), StatusCode::OK);
    let bytes_put = resp_put.into_body().collect().await.unwrap().to_bytes();
    let draft_put: Value = serde_json::from_slice(&bytes_put).unwrap();
    assert_eq!(draft_put["name"], "feed-1");
    assert_eq!(draft_put["version"], 1);

    let resp_get = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/projects/ovzdusie/drafts/DataSource/feed-1")
                .header(header::COOKIE, &steward_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_get.status(), StatusCode::OK);
    let bytes_get = resp_get.into_body().collect().await.unwrap().to_bytes();
    let draft_get: Value = serde_json::from_slice(&bytes_get).unwrap();
    assert_eq!(draft_get["name"], "feed-1");
    assert_eq!(draft_get["version"], 1);

    let resp_list = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/projects/ovzdusie/drafts")
                .header(header::COOKIE, &steward_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_list.status(), StatusCode::OK);
    let bytes_list = resp_list.into_body().collect().await.unwrap().to_bytes();
    let list_val: Value = serde_json::from_slice(&bytes_list).unwrap();
    let items = list_val["items"].as_array().expect("items list");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "feed-1");

    let put_conflict = json!({
        "manifest": {
            "apiVersion": API_VERSION,
            "kind": "DataSource",
            "metadata": { "name": "feed-1" },
            "spec": { "type": "http", "http": { "url": "https://example.org" } }
        },
        "expectedVersion": 999
    });
    let resp_conflict = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/drafts/DataSource/feed-1")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&put_conflict).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_conflict.status(), StatusCode::CONFLICT);

    let resp_del = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/ovzdusie/drafts/DataSource/feed-1")
                .header(header::COOKIE, &steward_cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_del.status(), StatusCode::OK);

    let resp_after_del = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/projects/ovzdusie/drafts/DataSource/feed-1")
                .header(header::COOKIE, &steward_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_after_del.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sweep_removes_idle_drafts() {
    let store = DraftStore::new(None);
    let m = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "ds-old" },
        "spec": { "type": "http" }
    });

    store
        .put("p1", "DataSource", "ds-old", m, None, "steward", "person")
        .await
        .unwrap();
    assert!(store
        .get("p1", "DataSource", "ds-old")
        .await
        .unwrap()
        .is_some());

    let _ = store.sweep(Duration::from_millis(0)).await;
    assert!(store
        .get("p1", "DataSource", "ds-old")
        .await
        .unwrap()
        .is_none());
}

/// A person bound on `ovzdusie` to a role that reads Pipeline only.
fn reader_on_ovzdusie(state: &AppState) {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    state.mirror.upsert(common::envelope(
        "Role",
        "pipeline-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "pipeline-reader-ovzdusie",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "reader@hel.fi" }],
            "role": "pipeline-reader",
            "scope": { "project": "ovzdusie" },
        }),
    ));
}

/// PF-59, R20 (T-1407): the draft stream and the draft list are reads, so a project the caller
/// holds no binding in answers `404`, the answer of a project that does not exist; a draft write
/// there keeps its `403`, which names what is missing (PF-50).
#[tokio::test]
async fn drafts_of_a_project_the_caller_may_not_read_are_not_found() {
    let state = AppState::new(Config::for_tests(), None).with_mirror(Arc::new(Mirror::new()));
    reader_on_ovzdusie(&state);
    let stranger = || common::person("stranger");

    for uri in [
        "/api/v1/projects/ovzdusie/drafts/events",
        "/api/v1/projects/ovzdusie/drafts",
    ] {
        let answer = common::send(&state, stranger(), "GET", uri, None).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{uri}: {}",
            answer.text
        );
        let absent = common::send(
            &state,
            stranger(),
            "GET",
            &uri.replace("ovzdusie", "no-such-project"),
            None,
        )
        .await;
        assert_eq!(
            answer.text.replace("ovzdusie", "no-such-project"),
            absent.text,
            "{uri}: a project the caller may not read reads like one that is not there"
        );
    }

    let write = common::send(
        &state,
        stranger(),
        "PUT",
        "/api/v1/projects/ovzdusie/drafts/Pipeline/p1",
        Some(json!({ "manifest": {
            "apiVersion": API_VERSION, "kind": "Pipeline", "metadata": { "name": "p1" }, "spec": {}
        } })),
    )
    .await;
    assert_eq!(write.status, StatusCode::FORBIDDEN, "{}", write.text);
}

/// The refusal's other half: a person bound on the project reads its drafts and opens the stream.
#[tokio::test]
async fn a_person_bound_on_the_project_reads_its_drafts_and_its_stream() {
    let state = AppState::new(Config::for_tests(), None).with_mirror(Arc::new(Mirror::new()));
    reader_on_ovzdusie(&state);

    let list = common::send(
        &state,
        common::person("reader"),
        "GET",
        "/api/v1/projects/ovzdusie/drafts",
        None,
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text);

    // The stream never ends, so only its head is read.
    let config = state.config.clone();
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/drafts/events")
                .header(
                    header::COOKIE,
                    common::cookie(&config, common::person("reader")),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
}

/// A draft of `kind` named `name` in `ovzdusie`, in the space `space` when there is one.
async fn draft_of(state: &AppState, kind: &str, name: &str, space: Option<&str>) {
    let mut manifest = json!({
        "apiVersion": API_VERSION, "kind": kind, "metadata": { "name": name }, "spec": {}
    });
    if let Some(space) = space {
        manifest["spec"]["contextSpaceRef"] = json!(space);
    }
    joinedcontext_portal::ops::drafts::draft_store(state)
        .put("ovzdusie", kind, name, manifest, None, "author", "person")
        .await
        .expect("draft put");
}

/// Binds `who@hel.fi` to a role reading `kinds`, over `scope`.
fn bind_reader(state: &AppState, who: &str, kinds: Value, scope: Value) {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    state.mirror.upsert(common::envelope(
        "Role",
        &format!("{who}-role"),
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        &format!("{who}-binding"),
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": format!("{who}@hel.fi") }],
            "role": format!("{who}-role"),
            "scope": scope,
        }),
    ));
}

fn names(list: &str) -> Vec<String> {
    let body: Value = serde_json::from_str(list).expect("a JSON list");
    let mut names: Vec<String> = body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|d| {
            format!(
                "{}/{}",
                d["kind"].as_str().unwrap(),
                d["name"].as_str().unwrap()
            )
        })
        .collect();
    names.sort();
    names
}

/// PF-59 (T-1455): a draft is the next version of a manifest and says who is writing it, so it
/// is listed and read exactly where the manifest would be: by kind, and by space for a binding
/// scoped to one.
#[tokio::test]
async fn drafts_are_listed_and_read_by_the_kind_and_space_the_caller_may_read() {
    let state = AppState::new(Config::for_tests(), None).with_mirror(Arc::new(Mirror::new()));
    for space in ["vzduch", "doprava"] {
        state.mirror.upsert(common::envelope(
            "ContextSpace",
            space,
            "ovzdusie",
            json!({}),
        ));
    }
    bind_reader(
        &state,
        "pipes",
        json!(["Pipeline"]),
        json!({ "project": "ovzdusie" }),
    );
    bind_reader(
        &state,
        "air",
        json!(["Pipeline", "Endpoint"]),
        json!({ "contextSpace": "vzduch" }),
    );
    draft_of(&state, "Role", "secret-admins", None).await;
    draft_of(&state, "Pipeline", "air-feed", Some("vzduch")).await;
    draft_of(&state, "Pipeline", "traffic-feed", Some("doprava")).await;
    draft_of(&state, "Endpoint", "air-public", Some("vzduch")).await;

    let list = |who: &'static str| {
        let state = state.clone();
        async move {
            let answer = common::send(
                &state,
                common::person(who),
                "GET",
                "/api/v1/projects/ovzdusie/drafts",
                None,
            )
            .await;
            assert_eq!(answer.status, StatusCode::OK, "{who}: {}", answer.text);
            names(&answer.text)
        }
    };
    assert_eq!(
        list("pipes").await,
        vec!["Pipeline/air-feed", "Pipeline/traffic-feed"]
    );
    assert_eq!(
        list("air").await,
        vec!["Endpoint/air-public", "Pipeline/air-feed"]
    );

    for (who, uri, status) in [
        (
            "pipes",
            "/api/v1/projects/ovzdusie/drafts/Role/secret-admins",
            StatusCode::NOT_FOUND,
        ),
        (
            "air",
            "/api/v1/projects/ovzdusie/drafts/Pipeline/traffic-feed",
            StatusCode::NOT_FOUND,
        ),
        (
            "air",
            "/api/v1/projects/ovzdusie/drafts/Pipeline/air-feed",
            StatusCode::OK,
        ),
    ] {
        let answer = common::send(&state, common::person(who), "GET", uri, None).await;
        assert_eq!(answer.status, status, "{who} {uri}: {}", answer.text);
        assert!(
            !answer.text.contains("author") || status == StatusCode::OK,
            "{who} {uri}"
        );
    }
}

/// The stream announces a draft only to a reader of its kind and space (T-1455): the Role draft
/// and the other space's pipeline pass by unseen, the pipeline of the reader's space arrives.
#[tokio::test]
async fn the_draft_stream_announces_only_what_the_reader_may_read() {
    let state = AppState::new(Config::for_tests(), None).with_mirror(Arc::new(Mirror::new()));
    for space in ["vzduch", "doprava"] {
        state.mirror.upsert(common::envelope(
            "ContextSpace",
            space,
            "ovzdusie",
            json!({}),
        ));
    }
    bind_reader(
        &state,
        "air",
        json!(["Pipeline"]),
        json!({ "contextSpace": "vzduch" }),
    );
    let config = state.config.clone();
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/drafts/events")
                .header(
                    header::COOKIE,
                    common::cookie(&config, common::person("air")),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    draft_of(&state, "Role", "secret-admins", None).await;
    draft_of(&state, "Pipeline", "traffic-feed", Some("doprava")).await;
    draft_of(&state, "Pipeline", "air-feed", Some("vzduch")).await;

    // Events arrive in order, so once the last one is read the two before it were filtered.
    let mut body = response.into_body();
    let mut seen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !seen.contains("air-feed") {
        let frame = tokio::time::timeout_at(deadline, body.frame())
            .await
            .expect("the reader's own pipeline is announced within 5 s")
            .expect("the stream stays open")
            .expect("a frame");
        if let Ok(data) = frame.into_data() {
            seen.push_str(&String::from_utf8_lossy(&data));
        }
    }
    assert!(
        !seen.contains("secret-admins"),
        "a Role draft reached a pipeline reader: {seen}"
    );
    assert!(
        !seen.contains("traffic-feed"),
        "another space's draft reached the reader: {seen}"
    );
}
