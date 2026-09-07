//! ServiceAccount API keys through the real router (T-0189, PF-36, PF-37, PF-38, PF-40).
//!
//! The database tests run when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may
//! write to (locally: `docker run -e POSTGRES_PASSWORD=… postgres:17-alpine`); the rest of the
//! file, the parts that decide who may manage an account, needs no database at all.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

const CSRF: &str = "csrf-token-value";
const OWNER: &str = "jana.kovacova";

fn session_cookie(config: &Config, username: &str, roles: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: Some(format!("{username}@banskabystrica.sk")),
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

fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "ServiceAccount".into(),
        metadata: ObjectMeta {
            name: "vendorx-parking-push".into(),
            namespace: Some("banskabystrica".into()),
            ..Default::default()
        },
        spec: json!({
            "owner": { "user": OWNER },
            "purpose": "VendorX pushes ParkingSpot updates every 10 s",
            "roles": [],
            "credentials": [
                { "kind": "oauth-client", "name": "main" },
                { "kind": "api-key", "name": "legacy-push" }
            ]
        }),
        status: None,
    });
    mirror
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
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn database_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// The router with a live database, and the very configuration it was built from: the session
/// cookies are encrypted with that config's key, and `Config::for_tests` mints a fresh key on
/// every call, so a second config would produce cookies this router cannot read.
async fn app_with_db() -> Option<(axum::Router, Config)> {
    let url = database_url()?;
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let config = Config::for_tests();
    let app = server::app(
        AppState::new(config.clone(), None)
            .with_mirror(mirror())
            .with_db(pool),
    );
    Some((app, config))
}

const KEYS: &str = "/api/v1/projects/banskabystrica/serviceaccounts/vendorx-parking-push/keys";

#[tokio::test]
async fn anonymous_calls_never_reach_the_key_store() {
    let app = server::app(AppState::new(Config::for_tests(), None).with_mirror(mirror()));
    let response = app
        .oneshot(Request::builder().uri(KEYS).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_stranger_is_told_the_account_does_not_exist() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config, "peter.novak", &[]);
    let app = server::app(AppState::new(config, None).with_mirror(mirror()));

    let (status, body) = call(&app, &cookie, Method::GET, KEYS, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an account a caller may not manage is not disclosed as existing (R20)"
    );
    assert_eq!(body["status"], 404);
}

#[tokio::test]
async fn an_unknown_account_and_a_forbidden_one_answer_the_same() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config, "peter.novak", &[]);
    let app = server::app(AppState::new(config, None).with_mirror(mirror()));

    let (forbidden, _) = call(&app, &cookie, Method::GET, KEYS, None).await;
    let (unknown, _) = call(
        &app,
        &cookie,
        Method::GET,
        "/api/v1/projects/banskabystrica/serviceaccounts/invented/keys",
        None,
    )
    .await;
    assert_eq!(forbidden, unknown);
}

#[tokio::test]
async fn without_a_database_the_owner_gets_503_and_not_an_empty_list() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config, OWNER, &[]);
    let app = server::app(AppState::new(config, None).with_mirror(mirror()));

    let (status, body) = call(&app, &cookie, Method::GET, KEYS, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], 503);
}

#[tokio::test]
async fn the_raw_key_is_returned_once_and_never_again() {
    let Some((app, config)) = app_with_db().await else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let cookie = session_cookie(&config, OWNER, &[]);

    let (status, minted) = call(
        &app,
        &cookie,
        Method::POST,
        KEYS,
        Some(json!({ "credential": "legacy-push" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let token = minted["token"].as_str().expect("a token, once").to_string();
    let key_id = minted["keyId"].as_str().expect("a key id").to_string();
    let secret = token
        .strip_prefix(&format!("jc_{key_id}_"))
        .expect("the token is jc_{keyId}_{secret} (PF-37)");
    assert!(secret.len() >= 40, "32 random bytes in base64: {secret}");

    let (status, listed) = call(&app, &cookie, Method::GET, KEYS, None).await;
    assert_eq!(status, StatusCode::OK);
    let items = listed["items"].as_array().expect("a list");
    let mine = items
        .iter()
        .find(|item| item["keyId"] == key_id.as_str())
        .expect("the key is listed");
    assert!(
        mine.get("token").is_none(),
        "the token is never listed again"
    );
    assert!(
        !listed.to_string().contains(&token),
        "neither the token nor its hash may appear in a listing"
    );
    assert_eq!(mine["credential"], "legacy-push");
}

#[tokio::test]
async fn a_key_for_an_undeclared_credential_is_refused() {
    let Some((app, config)) = app_with_db().await else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let cookie = session_cookie(&config, OWNER, &[]);

    for credential in ["main", "invented"] {
        let (status, body) = call(
            &app,
            &cookie,
            Method::POST,
            KEYS,
            Some(json!({ "credential": credential })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "'{credential}' is not an api-key credential of the manifest"
        );
        assert_eq!(body["status"], 400);
    }

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        KEYS,
        Some(json!({ "credential": "legacy-push", "expiresAt": "2020-01-01T00:00:00Z" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "an expiry in the past");
}

#[tokio::test]
async fn rotation_keeps_both_keys_alive_and_revocation_ends_one_now() {
    let Some((app, config)) = app_with_db().await else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    // Not the owner: the platform administrator's realm role is the other way in (CC-60).
    let cookie = session_cookie(&config, "marek.mraz", &["portal-approver"]);

    let (_, first) = call(
        &app,
        &cookie,
        Method::POST,
        KEYS,
        Some(json!({ "credential": "legacy-push" })),
    )
    .await;
    let old_id = first["keyId"].as_str().expect("a key id").to_string();

    let (status, second) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("{KEYS}/{old_id}/rotate"),
        Some(json!({ "overlapHours": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let new_id = second["keyId"].as_str().expect("a successor").to_string();
    assert_ne!(new_id, old_id);
    assert_ne!(second["token"], first["token"]);

    let (_, listed) = call(&app, &cookie, Method::GET, KEYS, None).await;
    let items = listed["items"].as_array().expect("a list");
    let old = items
        .iter()
        .find(|item| item["keyId"] == old_id.as_str())
        .expect("the predecessor is still listed");
    assert!(
        old["expiresAt"].is_string(),
        "the rotated key now ends, but only when the overlap does (PF-38)"
    );
    assert!(old["revokedAt"].is_null() || old.get("revokedAt").is_none());
    assert!(items.iter().any(|item| item["keyId"] == new_id.as_str()));

    let (status, _) = call(
        &app,
        &cookie,
        Method::DELETE,
        &format!("{KEYS}/{new_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, listed) = call(&app, &cookie, Method::GET, KEYS, None).await;
    let revoked = listed["items"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|item| item["keyId"] == new_id.as_str())
        .expect("a revoked key keeps its row for the audit trail");
    assert!(revoked["revokedAt"].is_string());

    let (status, _) = call(
        &app,
        &cookie,
        Method::DELETE,
        &format!("{KEYS}/deadbeefdeadbeef"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "an unknown key id");
}

#[tokio::test]
async fn a_write_without_the_csrf_token_is_refused() {
    let Some((app, config)) = app_with_db().await else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let cookie = session_cookie(&config, OWNER, &[]);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(KEYS)
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "credential": "legacy-push" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
