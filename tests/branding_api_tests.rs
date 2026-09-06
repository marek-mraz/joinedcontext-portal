//! `GET /api/v1/branding`: one image, many installations (T-0340, UI-30, OPS-46).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

const BLOCK: &str = r##"
instanceName: "Helsinki Region Context"
organisation: "City of Helsinki"
contactEmail: "opendata@hel.fi"
logo: "logo.svg"
colours:
  primary: "#0000bf"
  secondary: "#0072c6"
  accent: "#ffe977"
  background: "#ffffff"
  text: "#1a1a1a"
languages:
  default: "fi"
  offered: ["fi", "sv", "en"]
"##;

async fn branding(file: Option<&str>) -> (StatusCode, Option<String>, Value) {
    let mut config = Config::for_tests();
    config.branding_file = file.map(str::to_owned);
    let app = server::app(AppState::new(config, None));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    let status = response.status();
    let cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, cache, serde_json::from_slice(&bytes).expect("json"))
}

fn written(contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "jc-branding-api-{}-{}.yaml",
        std::process::id(),
        contents.len()
    ));
    std::fs::write(&path, contents).expect("write the branding file");
    path
}

/// The login page needs the name and the logo before anyone has signed in.
#[tokio::test]
async fn the_branding_is_public_and_cacheable() {
    let path = written(BLOCK);
    let (status, cache, body) = branding(Some(&path.display().to_string())).await;
    let _ = std::fs::remove_file(&path);

    assert_eq!(status, StatusCode::OK, "no session, no redirect: {body}");
    assert_eq!(
        cache.as_deref(),
        Some("public, max-age=300"),
        "the one API answer a browser may keep"
    );
    assert_eq!(body["instanceName"], json!("Helsinki Region Context"));
    assert_eq!(body["organisation"], json!("City of Helsinki"));
    assert_eq!(body["colours"]["primary"], json!("#0000bf"));
    assert_eq!(body["languages"]["default"], json!("fi"));
    assert_eq!(body["languages"]["offered"], json!(["fi", "sv", "en"]));
    assert_eq!(body["logo"], json!("logo.svg"));
}

/// An installation whose ConfigMap has not been rendered looks plain; it does not break.
#[tokio::test]
async fn an_installation_without_branding_serves_neutral_defaults() {
    let (status, _, body) = branding(None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["instanceName"], json!("joinedcontext"));
    assert_eq!(body["colours"]["primary"], json!("#1d4ed8"));

    let (status, _, body) = branding(Some("/nonexistent/jc/branding.yaml")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["instanceName"], json!("joinedcontext"));
}

/// OPS-46: what reaches a CSS custom property is a colour or the default, never a value the
/// browser would evaluate as something else.
#[tokio::test]
async fn a_colour_that_is_not_a_colour_never_reaches_the_browser() {
    let path = written(
        "instanceName: \"Injected\"\ncolours:\n  primary: \"red; background: url(https://evil.example/x)\"\n",
    );
    let (status, _, body) = branding(Some(&path.display().to_string())).await;
    let _ = std::fs::remove_file(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["colours"]["primary"], json!("#1d4ed8"));
    assert!(
        !body.to_string().contains("evil.example"),
        "the rejected value must not be echoed either: {body}"
    );
}

/// The logo comes from the platform's own origin, so a page never fetches a third party.
#[tokio::test]
async fn the_logo_is_served_from_beside_the_branding_file() {
    let dir = std::env::temp_dir().join(format!("jc-branding-assets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the asset directory");
    let file = dir.join("branding.yaml");
    std::fs::write(&file, "instanceName: \"Helsinki\"\nlogo: \"logo.svg\"\n").expect("branding");
    std::fs::write(
        dir.join("logo.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .expect("logo");

    let mut config = Config::for_tests();
    config.branding_file = Some(file.display().to_string());
    let app = server::app(AppState::new(config, None));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding/logo")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/svg+xml")
    );

    // Nothing is configured as a favicon, so there is nothing to serve.
    let missing = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding/favicon")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file name that would leave the ConfigMap was already dropped, so nothing can be reached
/// through this route but the two assets the branding block names.
#[tokio::test]
async fn an_asset_outside_the_branding_directory_is_not_served() {
    let dir = std::env::temp_dir().join(format!("jc-branding-escape-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the asset directory");
    let file = dir.join("branding.yaml");
    std::fs::write(&file, "logo: \"../../etc/passwd\"\n").expect("branding");

    let mut config = Config::for_tests();
    config.branding_file = Some(file.display().to_string());
    let app = server::app(AppState::new(config, None));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding/logo")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let _ = std::fs::remove_dir_all(&dir);
}
