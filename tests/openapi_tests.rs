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
