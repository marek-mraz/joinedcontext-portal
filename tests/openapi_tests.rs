use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tower::ServiceExt;

#[tokio::test]
async fn openapi_spec_served_correctly() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.starts_with("application/json"));

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let doc: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    let openapi_ver = doc["openapi"].as_str().expect("openapi version string");
    assert!(
        openapi_ver.starts_with("3.1"),
        "expected openapi 3.1, got {openapi_ver}"
    );

    assert!(
        doc["paths"]["/api/v1/health"].is_object(),
        "expected /api/v1/health in paths, got: {:?}",
        doc["paths"]
    );

    assert_eq!(doc["info"]["title"], "joinedcontext Portal API");

    let schemas = &doc["components"]["schemas"];
    assert!(schemas["Health"].is_object(), "missing Health schema");
    assert!(
        schemas["ProblemDetails"].is_object(),
        "missing ProblemDetails schema"
    );
}

use std::path::PathBuf;

use joinedcontext_portal::openapi::ApiDoc;
use utoipa::OpenApi;

fn spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/openapi.json")
}

fn rendered() -> String {
    let mut json = serde_json::to_string_pretty(&ApiDoc::openapi()).expect("serialise the spec");
    json.push('\n');
    json
}

/// The UI generates its types from the committed spec, so CI never needs a running server.
/// Regenerate with `cargo test --test openapi_tests -- --ignored write_openapi_json`,
/// then `pnpm generate:api` in `ui/`.
#[test]
fn committed_openapi_spec_is_current() {
    let committed = std::fs::read_to_string(spec_path()).expect("ui/openapi.json is committed");
    assert_eq!(
        committed,
        rendered(),
        "ui/openapi.json is stale — rerun the writer test and `pnpm generate:api`"
    );
}

#[test]
#[ignore = "writes ui/openapi.json; run explicitly after changing the API surface"]
fn write_openapi_json() {
    std::fs::write(spec_path(), rendered()).expect("write ui/openapi.json");
}

#[test]
fn every_documented_path_is_versioned_and_not_a_kubernetes_apis_path() {
    let spec = ApiDoc::openapi();
    for path in spec.paths.paths.keys() {
        assert!(
            path.starts_with("/api/v1/"),
            "path {path} is not under /api/v1/"
        );
        assert!(
            !path.starts_with("/apis/"),
            "path {path} uses the k8s shape"
        );
    }
}
