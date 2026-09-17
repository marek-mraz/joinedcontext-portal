//! The static apps host: what it serves, what it refuses, and the headers every app carries
//! (AP-12, AP-14, AP-17).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::apps::static_host::sri_sha384;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;

const INDEX: &[u8] = b"<!doctype html><title>air quality</title>";
const BUNDLE_JS: &[u8] = b"console.log('air quality');";

/// One app directory with its bundle and the integrity manifest CI writes beside it.
fn app_root(case: &str, files: &[(&str, &[u8])]) -> tempdir::Dir {
    let dir = tempdir::Dir::new(case);
    let app = dir.path().join("air-quality");
    std::fs::create_dir_all(&app).expect("app dir");
    let mut digests = serde_json::Map::new();
    for (name, bytes) in files {
        std::fs::write(app.join(name), bytes).expect("bundle file");
        digests.insert((*name).into(), sri_sha384(bytes).into());
    }
    std::fs::write(
        app.join("integrity.json"),
        serde_json::to_vec(&digests).expect("manifest"),
    )
    .expect("write manifest");
    dir
}

/// The tiniest temp directory that cleans up after itself; the suite needs nothing more.
mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new(case: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "jc-apps-{case}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default()
            ));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
}

/// The same mirror with the build the lane wrote back (AP-13a).
fn mirror_with_build(spec: serde_json::Value, commit: &str) -> Arc<Mirror> {
    let mirror = mirror_with_app(spec);
    let mut envelope = mirror
        .get("ovzdusie", "App", "air-quality")
        .expect("the app");
    envelope.status = Some(joinedcontext_portal::resource::Status {
        phase: joinedcontext_portal::resource::Phase::Live,
        observed_revision: None,
        source_url: None,
        conditions: Vec::new(),
        build: Some(jc_core::Build {
            digest: format!("sha256:{}", "a1b2c3d4".repeat(8)),
            commit: commit.to_owned(),
            sdk_version: "0.4.1".to_owned(),
            built_at: chrono::Utc::now(),
        }),
    });
    mirror.upsert(envelope);
    mirror
}

fn mirror_with_app(spec: serde_json::Value) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "App".into(),
        metadata: ObjectMeta {
            name: "air-quality".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        },
        spec,
        status: None,
    });
    mirror
}

fn app_spec(lifecycle: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "static",
        "source": { "path": "." },
        "build": { "node": "22" },
        "visibility": "public",
        "lifecycle": lifecycle,
        "dataNeeds": []
    })
}

async fn get_from(
    root: &std::path::Path,
    spec: serde_json::Value,
    uri: &str,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let config = Config {
        apps_dir: Some(root.to_string_lossy().into_owned()),
        ..Config::for_tests()
    };
    let app = server::app(AppState::new(config, None).with_mirror(mirror_with_app(spec)));
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, body.to_vec())
}

#[tokio::test]
async fn a_published_app_serves_its_bundle_under_the_platform_host() {
    let dir = app_root("serves", &[("index.html", INDEX), ("bundle.js", BUNDLE_JS)]);

    let (status, headers, body) =
        get_from(dir.path(), app_spec("published"), "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INDEX, "the app's own index, not the Portal SPA");
    assert!(headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));

    let (status, headers, body) = get_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/bundle.js",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, BUNDLE_JS);
    assert!(headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .contains("javascript"));
}

#[tokio::test]
async fn every_app_response_carries_its_own_policy_and_not_the_portals() {
    let dir = app_root("csp", &[("index.html", INDEX)]);

    let (_, headers, _) = get_from(dir.path(), app_spec("published"), "/apps/air-quality/").await;

    let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert!(csp.contains("connect-src 'self';"), "{csp}");
    assert!(
        !csp.contains("worker-src"),
        "the Portal's policy is not the app's: {csp}"
    );
    assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY");
    assert_eq!(headers["x-content-type-options"], "nosniff");
}

#[tokio::test]
async fn an_embeddable_app_may_be_framed_by_the_named_origin() {
    let dir = app_root("embed", &[("index.html", INDEX)]);
    let mut spec = app_spec("published");
    spec["embeddable"] = serde_json::json!(true);
    spec["csp"] = serde_json::json!({ "frameAncestors": ["https://portal.example.sk"] });

    let (_, headers, _) = get_from(dir.path(), spec, "/apps/air-quality/").await;

    let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(
        csp.contains("frame-ancestors https://portal.example.sk"),
        "{csp}"
    );
    assert_eq!(headers[header::X_FRAME_OPTIONS], "SAMEORIGIN");
}

#[tokio::test]
async fn an_unpublished_app_is_not_found_rather_than_forbidden() {
    // A draft, a preview and a retired app answer exactly what a name that never existed
    // answers, so the host discloses nothing about what is being worked on (AP-18).
    let dir = app_root("draft", &[("index.html", INDEX)]);

    for lifecycle in ["draft", "preview", "retired"] {
        let (status, _, _) = get_from(dir.path(), app_spec(lifecycle), "/apps/air-quality/").await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{lifecycle} must not be reachable"
        );
    }

    let (status, _, _) = get_from(dir.path(), app_spec("published"), "/apps/nothing-here/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_non_public_app_needs_a_session() {
    let dir = app_root("private", &[("index.html", INDEX)]);
    let mut spec = app_spec("published");
    spec["visibility"] = serde_json::json!("project");

    let (status, _, _) = get_from(dir.path(), spec, "/apps/air-quality/").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_modified_asset_is_refused_rather_than_served() {
    let dir = app_root(
        "tampered",
        &[("index.html", INDEX), ("bundle.js", BUNDLE_JS)],
    );
    // The digest manifest is what CI recorded; the file on disk changed afterwards.
    std::fs::write(
        dir.path().join("air-quality/bundle.js"),
        b"console.log('exfiltrate');",
    )
    .expect("tamper");

    let (status, _, body) = get_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/bundle.js",
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !String::from_utf8_lossy(&body).contains("exfiltrate"),
        "the modified bytes must never reach the caller"
    );
}

#[tokio::test]
async fn a_file_without_a_recorded_digest_is_not_part_of_the_bundle() {
    let dir = app_root("stray", &[("index.html", INDEX)]);
    std::fs::write(dir.path().join("air-quality/notes.txt"), b"internal").expect("stray file");

    let (status, _, _) = get_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/notes.txt",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_bundle_without_an_integrity_manifest_serves_nothing() {
    let dir = tempdir::Dir::new("no-manifest");
    let app = dir.path().join("air-quality");
    std::fs::create_dir_all(&app).expect("app dir");
    std::fs::write(app.join("index.html"), INDEX).expect("index");

    let (status, _, _) = get_from(dir.path(), app_spec("published"), "/apps/air-quality/").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_traversal_never_reads_outside_the_app_directory() {
    let dir = app_root("traversal", &[("index.html", INDEX)]);
    std::fs::write(dir.path().join("secret.txt"), b"another app's file").expect("neighbour");

    for uri in [
        "/apps/air-quality/../secret.txt",
        "/apps/air-quality/..%2fsecret.txt",
        "/apps/air-quality/%2e%2e/secret.txt",
    ] {
        let (status, _, body) = get_from(dir.path(), app_spec("published"), uri).await;
        assert_ne!(status, StatusCode::OK, "{uri} was served");
        assert!(
            !String::from_utf8_lossy(&body).contains("another app's file"),
            "{uri} read outside the root"
        );
    }
}

#[tokio::test]
async fn an_unknown_path_inside_a_published_app_is_not_the_index() {
    // No SPA fallback: a missing asset stays visible as missing (AP-14).
    let dir = app_root("no-fallback", &[("index.html", INDEX)]);

    let (status, _, body) = get_from(
        dir.path(),
        app_spec("published"),
        "/apps/air-quality/dashboard",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_ne!(body, INDEX);
}

#[tokio::test]
async fn without_an_apps_directory_the_host_answers_not_found() {
    let app = server::app(
        AppState::new(Config::for_tests(), None)
            .with_mirror(mirror_with_app(app_spec("published"))),
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/apps/air-quality/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// One build directory of an app: `{apps_dir}/{name}/{commit}/` with its own integrity manifest
/// (AP-74), beside the previous publish at `{apps_dir}/{name}/`.
fn build_dir(dir: &tempdir::Dir, commit: &str, files: &[(&str, &[u8])]) {
    let app = dir.path().join("air-quality").join(commit);
    std::fs::create_dir_all(&app).expect("build dir");
    let mut digests = serde_json::Map::new();
    for (name, bytes) in files {
        std::fs::write(app.join(name), bytes).expect("bundle file");
        digests.insert((*name).into(), sri_sha384(bytes).into());
    }
    std::fs::write(
        app.join("integrity.json"),
        serde_json::to_vec(&digests).expect("manifest"),
    )
    .expect("write manifest");
}

async fn get_with(root: &std::path::Path, mirror: Arc<Mirror>, uri: &str) -> (StatusCode, Vec<u8>) {
    let config = Config {
        apps_dir: Some(root.to_string_lossy().into_owned()),
        ..Config::for_tests()
    };
    let app = server::app(AppState::new(config, None).with_mirror(mirror));
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

const NEXT_INDEX: &[u8] = b"<!doctype html><title>air quality, next build</title>";

/// AP-72, AP-74: the host serves the build the manifest names, not whatever sits in the app's
/// directory.
#[tokio::test]
async fn the_host_serves_the_build_the_manifest_names() {
    let dir = app_root("named-build", &[("index.html", INDEX)]);
    build_dir(&dir, "8c56954a1f0e", &[("index.html", NEXT_INDEX)]);

    let (status, body) = get_with(
        dir.path(),
        mirror_with_build(app_spec("published"), "8c56954a1f0e"),
        "/apps/air-quality/",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, NEXT_INDEX, "the named build, not the previous one");
}

/// AP-72: a build this host does not hold keeps the previous one serving, and the app is named
/// as missing its build rather than quietly serving something older as if it were current.
#[tokio::test]
async fn a_build_the_host_does_not_hold_keeps_the_previous_one_serving_and_is_reported() {
    let dir = app_root("missing-build", &[("index.html", INDEX)]);
    let mirror = mirror_with_build(app_spec("published"), "0000000feed");

    let (status, body) = get_with(dir.path(), mirror.clone(), "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, INDEX, "the previous publish keeps serving");

    let apps_dir = dir.path().to_string_lossy().into_owned();
    assert_eq!(
        joinedcontext_portal::apps::static_host::build_missing(Some(&apps_dir), &mirror),
        vec!["air-quality".to_owned()],
        "the app says its build never arrived"
    );

    // The build arrives: nothing else changes and the host follows it.
    build_dir(&dir, "0000000feed", &[("index.html", NEXT_INDEX)]);
    let (status, body) = get_with(dir.path(), mirror.clone(), "/apps/air-quality/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, NEXT_INDEX);
    assert!(
        joinedcontext_portal::apps::static_host::build_missing(Some(&apps_dir), &mirror).is_empty(),
        "nothing is missing once the build is there"
    );
}
