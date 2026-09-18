//! `status.build` has one writer: the build lane, the role whose `propose` on `App` is
//! constrained to that field (AP-13a, AP-73). Everything else about `status` is the platform's
//! own computation and no manifest carries it (MF-04).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::MockServer;

use common::{checked_send as send, envelope, forge, person};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const APPS: &str = "/api/v1/projects/ovzdusie/apps";

/// `builder@hel.fi` is the build lane: propose on App, constrained to `status.build`.
/// `jana@hel.fi` proposes Apps like anybody else.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(envelope(
        "Role",
        "app-builder",
        ORG_NAMESPACE,
        json!({ "rules": [{
            "kinds": ["App"],
            "verbs": ["propose"],
            "constraints": [{ "field": "status.build" }],
        }]}),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "app-editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["App"], "verbs": ["propose"] }] }),
    ));
    for (who, role) in [("builder", "app-builder"), ("jana", "app-editor")] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{who}-{role}"),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{who}@hel.fi") }],
                "role": role,
                "scope": { "project": "ovzdusie" },
            }),
        ));
    }
    state
}

fn app(status: Option<Value>, annotation: Option<&str>) -> Value {
    let mut metadata = json!({ "name": "air-quality", "namespace": "ovzdusie" });
    if let Some(key) = annotation {
        metadata["annotations"] = json!({ key: "sha256:deadbeef" });
    }
    let mut manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "App",
        "metadata": metadata,
        "spec": {
            "kind": "static",
            "source": { "path": "./src" },
            "build": { "node": "22" },
            "visibility": "public",
            "lifecycle": "published",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                "types": ["AirQualityObserved"],
                "operations": ["queryEntity"],
            }],
        },
    });
    if let Some(status) = status {
        manifest["status"] = status;
    }
    manifest
}

fn build() -> Value {
    json!({ "build": {
        "digest": format!("sha256:{}", "a1b2c3d4".repeat(8)),
        "commit": "8c56954a1f0e",
        "sdkVersion": "0.4.1",
        "builtAt": "2026-09-17T06:00:00Z",
    }})
}

/// What the merge request wrote, decoded.
async fn committed(gitea: &MockServer) -> Vec<String> {
    use base64::Engine;
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "PUT" && r.url.path().contains("/contents/"))
        .filter_map(|r| {
            let body: Value = serde_json::from_slice(&r.body).ok()?;
            let encoded = body.get("content")?.as_str()?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()?;
            String::from_utf8(bytes).ok()
        })
        .collect()
}

#[tokio::test]
async fn only_the_build_lane_writes_the_build_and_the_refusal_says_whose_field_it_is() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let refused = send(
        &state,
        person("jana"),
        "POST",
        APPS,
        Some(app(Some(build()), None)),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(refused.text.contains("build lane"), "{}", refused.text);
    assert!(
        committed(&gitea).await.is_empty(),
        "nothing was written for a refused proposal"
    );

    let accepted = send(
        &state,
        person("builder"),
        "POST",
        APPS,
        Some(app(Some(build()), None)),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let written = committed(&gitea).await;
    assert!(
        written.iter().any(|file| file.contains("status:")
            && file.contains("digest:")
            && file.contains("8c56954a1f0e")),
        "the build the lane wrote is what lands in the repository: {written:?}"
    );
}

#[tokio::test]
async fn every_other_part_of_status_is_refused_even_from_the_build_lane() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    for status in [
        json!({ "phase": "Live" }),
        json!({ "build": null }),
        json!({ "phase": "Live", "build": build()["build"] }),
    ] {
        let refused = send(
            &state,
            person("builder"),
            "POST",
            APPS,
            Some(app(Some(status.clone()), None)),
        )
        .await;
        assert_eq!(
            refused.status,
            StatusCode::BAD_REQUEST,
            "{status}: {}",
            refused.text
        );
        assert!(refused.text.contains("MF-04"), "{}", refused.text);
    }
}

/// AP-13a: a digest in an annotation is a digest somebody typed, whoever they are.
#[tokio::test]
async fn an_image_annotation_is_refused_from_the_build_lane_too() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    for key in ["joinedcontext.com/image", "joinedcontext.com/module"] {
        let refused = send(
            &state,
            person("builder"),
            "POST",
            APPS,
            Some(app(Some(build()), Some(key))),
        )
        .await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
        assert!(refused.text.contains(key), "{}", refused.text);
    }
}

/// A `status.build` on any other kind is the platform's computation, whoever asks.
#[tokio::test]
async fn a_build_on_another_kind_is_refused() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let refused = send(
        &state,
        person("builder"),
        "POST",
        "/api/v1/projects/ovzdusie/spaces",
        Some(json!({
            "apiVersion": API_VERSION,
            "kind": "ContextSpace",
            "metadata": { "name": "mhd", "namespace": "ovzdusie" },
            "spec": { "isSandbox": false },
            "status": build(),
        })),
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
}
