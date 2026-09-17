//! `Group` through the Portal's own doors (T-0867, PF-62, PF-64, PF-52).
//!
//! The kind is organization-level, so it is read and written under the `org` namespace like
//! `Role` and `RoleBinding`: a caller who may read it sees the members, one who may not sees a
//! project that is not there, proposing needs `propose` on `Group`, and a binding to a group no
//! manifest declares is refused before a change exists.

mod common;

use axum::http::StatusCode;
use common::{envelope, person, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use serde_json::{json, Value};
use wiremock::matchers::{method as http_method, path as url_path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A forge that takes the write and answers the open-pull list.
async fn forge() -> MockServer {
    let gitea = common::forge().await;
    Mock::given(http_method("GET"))
        .and(url_path(format!("{REPO}/pulls")))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&gitea)
        .await;
    gitea
}

/// A role at organization scope with these verbs on `Group`, bound to `who`.
fn bound(state: &joinedcontext_portal::state::AppState, who: &str, verbs: Value) {
    state.mirror.upsert(envelope(
        "Role",
        "group-keeper",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Group"], "verbs": verbs }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "keepers",
        ORG_NAMESPACE,
        json!({
            "role": "group-keeper",
            "subjects": [{ "user": format!("{who}@hel.fi") }],
            "scope": { "organization": "bb" }
        }),
    ));
}

fn group(name: &str, members: &[&str]) -> Value {
    json!({
        "apiVersion": joinedcontext_portal::resource::API_VERSION,
        "kind": "Group",
        "metadata": { "name": name, "namespace": ORG_NAMESPACE },
        "spec": {
            "description": "The people who lead the city's projects",
            "members": members.iter().map(|m| json!({ "user": m })).collect::<Vec<_>>()
        }
    })
}

#[tokio::test]
async fn a_reader_sees_the_members_and_a_stranger_sees_no_such_plural() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "Group",
        "city-leads",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": "lead@hel.fi" }] }),
    ));
    bound(&state, "keeper", json!(["read"]));

    let answer = common::send(
        &state,
        person("keeper"),
        "GET",
        "/api/v1/projects/org/groups",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(answer.text.contains("lead@hel.fi"), "{}", answer.text);

    let one = common::send(
        &state,
        person("keeper"),
        "GET",
        "/api/v1/projects/org/groups/city-leads",
        None,
    )
    .await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);

    // No binding at all: the kind is not there, not forbidden (PF-59, R20).
    let stranger = common::send(
        &state,
        person("nobody"),
        "GET",
        "/api/v1/projects/org/groups",
        None,
    )
    .await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
    assert!(!stranger.text.contains("lead@hel.fi"), "{}", stranger.text);
}

#[tokio::test]
async fn proposing_a_group_needs_propose_on_group_and_lands_in_the_red_lane() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    bound(&state, "keeper", json!(["read"]));

    // Read is not write, and the refusal says which verb is missing (PF-50).
    let refused = common::send(
        &state,
        person("keeper"),
        "POST",
        "/api/v1/projects/org/groups",
        Some(group("city-leads", &["lead@hel.fi"])),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(
        refused.text.contains("propose") && refused.text.contains("Group"),
        "{}",
        refused.text
    );

    bound(&state, "keeper", json!(["read", "propose"]));
    let accepted = common::send(
        &state,
        person("keeper"),
        "POST",
        "/api/v1/projects/org/groups",
        Some(group("city-leads", &["lead@hel.fi"])),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let change: Value = serde_json::from_str(&accepted.text).expect("a change");
    // Membership is reviewed like a Role and a RoleBinding (PF-52).
    assert_eq!(change["status"]["lane"], "red", "{}", accepted.text);
}

#[tokio::test]
async fn a_binding_to_a_group_no_manifest_declares_is_refused_before_a_change_exists() {
    let gitea = forge().await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Group", "RoleBinding", "Role"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "admins",
        ORG_NAMESPACE,
        json!({
            "role": "admin",
            "subjects": [{ "user": "keeper@hel.fi" }],
            "scope": { "organization": "bb" }
        }),
    ));
    let binding = |group: &str| {
        json!({
            "apiVersion": joinedcontext_portal::resource::API_VERSION,
            "kind": "RoleBinding",
            "metadata": { "name": "leads-admin", "namespace": ORG_NAMESPACE },
            "spec": {
                "role": "admin",
                "subjects": [{ "group": group }],
                "scope": { "organization": "bb" }
            }
        })
    };

    let refused = common::send(
        &state,
        person("keeper"),
        "POST",
        "/api/v1/projects/org/rolebindings",
        Some(binding("city-leads")),
    )
    .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    assert!(
        refused.text.contains("city-leads") && refused.text.contains("no Group manifest"),
        "the refusal says what to do about it: {}",
        refused.text
    );

    // With the group declared, the same binding is taken.
    state.mirror.upsert(envelope(
        "Group",
        "city-leads",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": "lead@hel.fi" }] }),
    ));
    let accepted = common::send(
        &state,
        person("keeper"),
        "POST",
        "/api/v1/projects/org/rolebindings",
        Some(binding("city-leads")),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
}
