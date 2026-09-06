//! The app's own contract with its Endpoint (T-0310, AP-04, AP-28, AP-39, AP-40, GW10).
//!
//! Every case runs the real router against a stubbed endpoint, so what is asserted is the
//! request that actually leaves the pod: which URL, which token, which method. The point of
//! the app is that it adds nothing to the caller's authority, and that is only observable
//! from outside the process.

use std::sync::Arc;

use air_quality::{router, App, Config};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const STATION: &str = "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:station-01";
const BASE: &str = "/apps/air-quality/";
// Not shaped like a JWT on purpose: a fixture that merely looks like a credential stops the
// secret scan for no reason, and what these tests assert is that the value is carried through
// unchanged, not what is inside it.
const TOKEN: &str = "the-forwarded-access-token-of-demo-steward";

fn app_at(endpoint: &MockServer) -> axum::Router {
    router(Arc::new(App::new(Config {
        base_path: BASE.to_owned(),
        endpoint_url: format!("{}/", endpoint.uri()),
        anonymous: false,
    })))
}

fn entity() -> Value {
    json!({
        "id": STATION,
        "type": "AirQualityObserved",
        "name": { "type": "Property", "value": "Štiavničky" },
        "pm10": { "type": "Property", "value": 34.2, "observedAt": "2026-09-06T10:00:00Z" },
        "pm25": { "type": "Property", "value": 21.0 },
        "location": {
            "type": "GeoProperty",
            "value": { "type": "Point", "coordinates": [19.146, 48.736] }
        }
    })
}

/// A request as the sidecar would deliver it: the user's token and identity in headers.
fn signed_in(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("x-forwarded-access-token", TOKEN)
        .header("x-forwarded-email", "demo.steward@banskabystrica.sk")
        .header("x-forwarded-user", "demo.steward@banskabystrica.sk");
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("a request"),
        None => builder.body(Body::empty()).expect("a request"),
    }
}

fn anonymous(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(path);
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("a request"),
        None => builder.body(Body::empty()).expect("a request"),
    }
}

async fn call(app: axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.oneshot(request).await.expect("the app answers");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("a readable body")
        .to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Mocks the PDP question the app asks before it shows the note box.
async fn access_check(endpoint: &MockServer, decision: bool) {
    Mock::given(method("POST"))
        .and(path("/access/check"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "decision": decision })))
        .mount(endpoint)
        .await;
}

#[tokio::test]
async fn the_station_list_is_read_with_the_users_own_token_and_flattened_for_the_browser() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .and(query_param("type", "AirQualityObserved"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([entity()])))
        .expect(1)
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in("GET", &format!("{BASE}api/stations"), None),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let stations: Value = serde_json::from_str(&body).expect("a list");
    assert_eq!(stations[0]["id"], json!(STATION));
    assert_eq!(stations[0]["pm10"], json!(34.2));
    assert_eq!(stations[0]["coordinates"], json!([19.146, 48.736]));
    assert_eq!(stations[0]["name"], json!("Štiavničky"));
}

/// AP-40, the property this whole app exists to demonstrate. Without a forwarded token there
/// is no write, and no second attempt without one either.
#[tokio::test]
async fn a_note_without_a_forwarded_token_is_401_and_never_reaches_the_endpoint() {
    let endpoint = MockServer::start().await;

    let (status, body) = call(
        app_at(&endpoint),
        anonymous(
            "POST",
            &format!("{BASE}api/stations/{STATION}/note"),
            Some(json!({ "note": "Sensor cleaned." })),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.contains("signed-in"), "{body}");
    assert!(
        endpoint
            .received_requests()
            .await
            .is_some_and(|r| r.is_empty()),
        "the app must not ask the endpoint on the caller's behalf"
    );
}

/// The write itself: one PATCH, one attribute, the user's token and nothing of the app's own.
#[tokio::test]
async fn a_steward_note_is_one_patch_of_one_attribute_with_the_forwarded_token() {
    let endpoint = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(format!("/ngsi-ld/v1/entities/{STATION}/attrs")))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .and(body_json(
            json!({ "stewardNote": { "type": "Property", "value": "Sensor cleaned." } }),
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "POST",
            &format!("{BASE}api/stations/{STATION}/note"),
            Some(json!({ "note": "  Sensor cleaned.  " })),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// GW10: the gateway refuses a signed-in viewer, and the app repeats the refusal rather than
/// paraphrasing it. A person who is told "you need the steward role" can act on that.
#[tokio::test]
async fn the_endpoints_refusal_reaches_the_browser_word_for_word() {
    let endpoint = MockServer::start().await;
    let problem = json!({
        "type": "https://joinedcontext.com/errors/forbidden",
        "title": "Forbidden",
        "status": 403,
        "detail": "writing stewardNote needs the project-steward role",
    });
    Mock::given(method("PATCH"))
        .and(path(format!("/ngsi-ld/v1/entities/{STATION}/attrs")))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(problem.clone())
                .insert_header("content-type", "application/problem+json"),
        )
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "POST",
            &format!("{BASE}api/stations/{STATION}/note"),
            Some(json!({ "note": "Sensor cleaned." })),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    let shown: Value = serde_json::from_str(&body).expect("a problem document");
    assert_eq!(shown, problem);
}

/// The note box follows the PDP, not a role read out of a token the app does not verify.
#[tokio::test]
async fn the_write_flag_comes_from_the_pdp_and_the_identity_from_the_sidecar() {
    let endpoint = MockServer::start().await;
    access_check(&endpoint, true).await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in("GET", &format!("{BASE}api/me"), None),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let me: Value = serde_json::from_str(&body).expect("an identity");
    assert_eq!(me["signedIn"], json!(true));
    assert_eq!(me["email"], json!("demo.steward@banskabystrica.sk"));
    assert_eq!(me["canWriteNote"], json!(true));
}

#[tokio::test]
async fn a_viewer_is_signed_in_and_still_has_no_note_box() {
    let endpoint = MockServer::start().await;
    access_check(&endpoint, false).await;

    let (_, body) = call(
        app_at(&endpoint),
        signed_in("GET", &format!("{BASE}api/me"), None),
    )
    .await;

    let me: Value = serde_json::from_str(&body).expect("an identity");
    assert_eq!(me["signedIn"], json!(true));
    assert_eq!(me["canWriteNote"], json!(false));
}

/// An anonymous reader of a public app never even asks: no token, no write, no round trip.
#[tokio::test]
async fn an_anonymous_reader_asks_the_pdp_nothing() {
    let endpoint = MockServer::start().await;

    let (_, body) = call(
        app_at(&endpoint),
        anonymous("GET", &format!("{BASE}api/me"), None),
    )
    .await;

    let me: Value = serde_json::from_str(&body).expect("an identity");
    assert_eq!(me["signedIn"], json!(false));
    assert_eq!(me["canWriteNote"], json!(false));
    assert!(endpoint
        .received_requests()
        .await
        .is_some_and(|r| r.is_empty()));
}

/// An id is a path segment upstream, so a caller cannot use it to reach another URL.
#[tokio::test]
async fn an_id_that_is_not_a_urn_is_refused_before_anything_leaves_the_pod() {
    let endpoint = MockServer::start().await;

    let (status, _) = call(
        app_at(&endpoint),
        signed_in(
            "POST",
            &format!("{BASE}api/stations/urn:ngsi-ld:x%2f..%2f..%2fadmin/note"),
            Some(json!({ "note": "hello" })),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(endpoint
        .received_requests()
        .await
        .is_some_and(|r| r.is_empty()));
}

#[tokio::test]
async fn an_empty_note_is_refused_by_the_app_and_not_by_the_broker() {
    let endpoint = MockServer::start().await;

    let (status, _) = call(
        app_at(&endpoint),
        signed_in(
            "POST",
            &format!("{BASE}api/stations/{STATION}/note"),
            Some(json!({ "note": "   " })),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(endpoint
        .received_requests()
        .await
        .is_some_and(|r| r.is_empty()));
}

#[tokio::test]
async fn the_history_asks_the_temporal_surface_for_the_last_day() {
    let endpoint = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/ngsi-ld/v1/temporal/entities/{STATION}")))
        .and(query_param("timerel", "after"))
        .and(query_param("attrs", "pm10,pm25"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": STATION })))
        .expect(1)
        .mount(&endpoint)
        .await;

    let (status, body) = call(
        app_at(&endpoint),
        signed_in(
            "GET",
            &format!("{BASE}api/stations/{STATION}/history"),
            None,
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The app lives under its base path and owns nothing above it: the sidecar routes one
/// prefix, and a request outside it is not this app's to answer.
#[tokio::test]
async fn nothing_is_served_above_the_apps_own_base_path() {
    let endpoint = MockServer::start().await;
    let (status, _) = call(app_at(&endpoint), anonymous("GET", "/api/stations", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for front in [BASE, BASE.trim_end_matches('/')] {
        let (status, body) = call(app_at(&endpoint), anonymous("GET", front, None)).await;
        assert_eq!(status, StatusCode::OK, "{front}");
        assert!(body.contains("<!doctype html>"), "{body}");
    }
}
