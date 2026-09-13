//! Tests for shared drafts, the draft store, and the draft REST API (AG-61, UI-47).

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
