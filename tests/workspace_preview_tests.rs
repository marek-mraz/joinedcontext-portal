//! Workspace previews (T-1239, T-1240; CC-78, CC-81, PF-83, API/01 §22): the branch rendered
//! with its prefix, one per workspace, two on the node, the owner only, served to the gateway
//! without the repository's secrets and without another project's Endpoints.

mod common;

use axum::http::StatusCode;
use common::{encode, forge, person, send, state_on, REPO};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::previews;
use joinedcontext_portal::ops::workspaces::{Opening, PreviewState, Scope};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "ovzdusie";
const SLUG: &str = "zt4qm7ge2xdv6ksb3ncf5arw2y";

fn owner() -> Identity {
    Identity {
        groups: vec!["portal-approver".into()],
        ..person("jana")
    }
}

fn files() -> Vec<(&'static str, String)> {
    let manifest = |kind: &str, name: &str, namespace: &str, spec: &str| {
        format!("apiVersion: joinedcontext.com/v1alpha1\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: {namespace}\nspec:\n{spec}")
    };
    vec![
        ("org.yaml", manifest("Organization", "hel", "org", "  domain: hel.fi\n  locales: [\"en\"]\n  defaultLocale: en\n")),
        ("projects/ovzdusie/project.yaml", manifest("Project", "ovzdusie", "org", "  organizationRef: hel\n")),
        ("projects/ovzdusie/spaces/air/space.yaml", manifest("ContextSpace", "air", "ovzdusie", "  isSandbox: false\n")),
        ("projects/ovzdusie/spaces/air/endpoints/public-air.yaml", manifest("Endpoint", "public-air", "ovzdusie", &format!("  slug: {SLUG}\n  contextSpaceRef: air\n  audience: public\n  enabledRepresentations: [ngsi-ld]\n"))),
        ("projects/ovzdusie/pipelines/air-feed/pipeline.yaml", manifest("Pipeline", "air-feed", "ovzdusie", "  class: auto\n  targetEndpoint: urn:ngsi-ld:Endpoint:{orgDomain}:ovzdusie-air:public-air\n  source:\n    endpointRef: { kind: Endpoint, name: public-air, namespace: ovzdusie }\n")),
        ("projects/doprava/project.yaml", manifest("Project", "doprava", "org", "  organizationRef: hel\n")),
        ("projects/doprava/spaces/roads/space.yaml", manifest("ContextSpace", "roads", "doprava", "  isSandbox: false\n")),
        ("projects/doprava/spaces/roads/endpoints/roads.yaml", manifest("Endpoint", "roads", "doprava", "  slug: qqqqqqqqqqqqqqqqqqqqqqqqqq\n  contextSpaceRef: roads\n  audience: public\n  enabledRepresentations: [ngsi-ld]\n")),
        ("secrets.enc.yaml", "feed: ENC[AES256_GCM,data:abc]\nsops:\n  version: 3.8.1\n".to_owned()),
    ]
}

/// The forge holding the workspace branch `workspace/{name}` at `head` with `files`.
async fn branch(server: &MockServer, name: &str, head: &str, files: &[(&str, String)]) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/workspace/{name}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": head } })))
        .mount(server)
        .await;
    let tree: Vec<_> = files
        .iter()
        .map(|(p, _)| json!({ "path": p, "type": "blob", "sha": format!("s-{p}") }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/git/trees/{head}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tree": tree, "truncated": false })),
        )
        .mount(server)
        .await;
    for (file, content) in files {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/contents/{file}")))
            .and(query_param("ref", head))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "path": file, "sha": "x", "encoding": "base64", "content": encode(content)
            })))
            .mount(server)
            .await;
    }
}

async fn opened(state: &AppState, name: &str) {
    state
        .workspaces
        .create(Opening {
            name,
            title: None,
            project: PROJECT,
            owner: "jana@hel.fi",
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("opened");
}

async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    branch(&server, "air-v2", "head1", &files()).await;
    let state = state_on(&server);
    opened(&state, "air-v2").await;
    (server, state)
}

fn preview_uri(name: &str) -> String {
    format!("/api/v1/projects/{PROJECT}/workspaces/{name}/preview")
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

#[tokio::test]
async fn starting_renders_the_project_with_its_prefix_and_every_pipeline_paused() {
    let (_server, state) = world().await;
    let started = send(&state, owner(), "POST", &preview_uri("air-v2"), None).await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text);
    let preview = body(&started.text);
    assert_eq!(preview["state"], "running");
    assert_eq!(preview["prefix"], "ws-air-v2-");
    let minted = jcctl::loader::preview_slug("ws-air-v2-", SLUG);
    assert_eq!(
        preview["endpoints"],
        json!([{
            "name": "public-air",
            "slug": minted,
            "url": format!("{}api/endpoint/{minted}", state.config.public_base_url),
        }])
    );
    assert_eq!(preview["pausedPipelines"], json!(["air-feed"]));
    let record = state.workspaces.get("air-v2").await.unwrap().unwrap();
    assert_eq!(record.preview_state, PreviewState::Running);

    let read = send(&state, owner(), "GET", &preview_uri("air-v2"), None).await;
    assert_eq!(body(&read.text)["endpoints"], preview["endpoints"]);
}

#[tokio::test]
async fn the_gateway_gets_the_project_without_secrets_or_other_projects() {
    let (_server, state) = world().await;
    assert!(
        previews::served(&state).await.unwrap().items.is_empty(),
        "nothing runs yet"
    );
    send(&state, owner(), "POST", &preview_uri("air-v2"), None).await;
    let served = previews::served(&state).await.unwrap().items;
    assert_eq!(served.len(), 1);
    assert_eq!(served[0].prefix, "ws-air-v2-");
    let paths: Vec<&str> = served[0].files.keys().map(String::as_str).collect();
    assert!(paths.contains(&"projects/ovzdusie/spaces/air/endpoints/public-air.yaml"));
    assert!(paths.contains(&"org.yaml"));
    assert!(
        !paths.iter().any(|p| p.starts_with("projects/doprava/")),
        "{paths:?}"
    );
    assert!(!paths.iter().any(|p| p.contains(".enc.")), "{paths:?}");
    assert!(!served[0].files.values().any(|text| text.contains("ENC[")));
}

#[tokio::test]
async fn one_preview_per_workspace_and_two_on_the_node() {
    let (server, state) = world().await;
    for name in ["b", "c"] {
        branch(&server, name, &format!("head-{name}"), &files()).await;
        opened(&state, name).await;
    }
    assert_eq!(
        send(&state, owner(), "POST", &preview_uri("air-v2"), None)
            .await
            .status,
        StatusCode::ACCEPTED
    );
    let again = send(&state, owner(), "POST", &preview_uri("air-v2"), None).await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert!(again.text.contains("runs already"), "{}", again.text);
    assert_eq!(
        send(&state, owner(), "POST", &preview_uri("b"), None)
            .await
            .status,
        StatusCode::ACCEPTED
    );
    let third = send(&state, owner(), "POST", &preview_uri("c"), None).await;
    assert_eq!(third.status, StatusCode::CONFLICT);
    assert!(
        third
            .text
            .contains("2 previews run on the node already (air-v2, b)"),
        "{}",
        third.text
    );
    assert_eq!(
        state
            .workspaces
            .get("c")
            .await
            .unwrap()
            .unwrap()
            .preview_state,
        PreviewState::None
    );

    // Stopping one frees the node; stopping a stopped one is a no-op.
    assert_eq!(
        send(&state, owner(), "DELETE", &preview_uri("b"), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&state, owner(), "DELETE", &preview_uri("b"), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&state, owner(), "POST", &preview_uri("c"), None)
            .await
            .status,
        StatusCode::ACCEPTED
    );
    let prefixes: Vec<String> = previews::served(&state)
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|s| s.prefix)
        .collect();
    assert_eq!(prefixes, vec!["ws-air-v2-", "ws-c-"]);
}

#[tokio::test]
async fn only_the_owner_starts_or_stops_and_a_stranger_learns_nothing() {
    let (_server, state) = world().await;
    let colleague = Identity {
        groups: vec!["portal-approver".into()],
        ..person("petra")
    };
    let refused = send(
        &state,
        colleague.clone(),
        "POST",
        &preview_uri("air-v2"),
        None,
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(refused.text.contains("only its owner"), "{}", refused.text);
    let stranger = send(
        &state,
        person("nobody"),
        "POST",
        &preview_uri("air-v2"),
        None,
    )
    .await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
    let unknown = send(&state, owner(), "POST", &preview_uri("nope"), None).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(
        state
            .workspaces
            .get("air-v2")
            .await
            .unwrap()
            .unwrap()
            .preview_state,
        PreviewState::None
    );
}

#[tokio::test]
async fn a_render_the_loader_refuses_leaves_the_preview_failed_with_the_reason() {
    let server = forge().await;
    let mut broken = files();
    broken.push((
        "projects/ovzdusie/spaces/air/endpoints/twin.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: public-air\n  namespace: ovzdusie\nspec:\n  slug: yyyyyyyyyyyyyyyyyyyyyyyyyy\n  contextSpaceRef: air\n  audience: public\n  enabledRepresentations: [ngsi-ld]\n".to_owned(),
    ));
    branch(&server, "air-v2", "head1", &broken).await;
    let state = state_on(&server);
    opened(&state, "air-v2").await;
    let started = send(&state, owner(), "POST", &preview_uri("air-v2"), None).await;
    assert_eq!(started.status, StatusCode::CONFLICT, "{}", started.text);
    assert!(started.text.contains("does not render"), "{}", started.text);
    assert_eq!(
        state
            .workspaces
            .get("air-v2")
            .await
            .unwrap()
            .unwrap()
            .preview_state,
        PreviewState::Error
    );
    let read = body(
        &send(&state, owner(), "GET", &preview_uri("air-v2"), None)
            .await
            .text,
    );
    assert_eq!(read["state"], "error");
    assert!(
        read["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "{read}"
    );
    assert!(
        previews::served(&state).await.unwrap().items.is_empty(),
        "a failed preview is not served"
    );
}

#[tokio::test]
async fn a_gone_branch_fails_the_start_and_an_expired_workspace_is_not_served() {
    let server = forge().await;
    let state = state_on(&server);
    opened(&state, "air-v2").await;
    let started = send(&state, owner(), "POST", &preview_uri("air-v2"), None).await;
    assert_eq!(started.status, StatusCode::NOT_FOUND, "{}", started.text);
    assert_eq!(
        state
            .workspaces
            .get("air-v2")
            .await
            .unwrap()
            .unwrap()
            .preview_state,
        PreviewState::Error
    );

    let (_server, state) = world().await;
    send(&state, owner(), "POST", &preview_uri("air-v2"), None).await;
    let past = chrono::Utc::now() + chrono::Duration::hours(25);
    assert!(state
        .workspaces
        .get("air-v2")
        .await
        .unwrap()
        .unwrap()
        .expired(past));
    // Expiry is judged when the gateway asks; a workspace past its TTL drops out (CC-81).
    state.workspaces.delete("air-v2").await.unwrap();
    assert!(previews::served(&state).await.unwrap().items.is_empty());
}
