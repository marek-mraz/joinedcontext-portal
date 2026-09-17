//! The reconciler owns the Keycloak groups the repository declares (T-0866, PF-62, PF-63).
//!
//! Against a mocked Keycloak admin API: a manifest with no group in the realm is created and
//! filled, a member nobody put in the manifest is pruned, what somebody changed in the console
//! is overwritten and reported as drift, a group without the managed attribute is never
//! touched, and a member the realm has no user for is a warning, not a failure.

use std::sync::Arc;

use joinedcontext_portal::reconciler::groups::{GroupSync, MANAGED_BY, MANAGED_VALUE};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REALM: &str = "/admin/realms/bb";

fn group(name: &str, members: &[&str]) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Group".to_owned(),
        metadata: ObjectMeta::new(name, joinedcontext_portal::permissions::ORG_NAMESPACE),
        spec: json!({ "members": members.iter().map(|m| json!({ "user": m })).collect::<Vec<_>>() }),
        status: Some(joinedcontext_portal::resource::Status {
            phase: joinedcontext_portal::resource::Phase::Live,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
        }),
    }
}

fn mirror_with(groups: Vec<ResourceEnvelope>) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    for envelope in groups {
        mirror.upsert(envelope);
    }
    mirror
}

/// A realm that hands out a token and answers the reads a run makes; each test mounts the
/// groups and users it wants on top.
async fn realm() -> MockServer {
    let keycloak = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "admin-token", "expires_in": 60 })),
        )
        .mount(&keycloak)
        .await;
    keycloak
}

fn sync(keycloak: &MockServer) -> GroupSync {
    GroupSync::new(
        &format!("{}/realms/bb", keycloak.uri()),
        "portal-reconciler".to_owned(),
        "s3cret".to_owned(),
    )
    .expect("a realm issuer")
}

/// What the run wrote, as `METHOD path`.
async fn wrote(keycloak: &MockServer) -> Vec<String> {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}", r.method.as_str(), r.url.path()))
        .collect()
}

#[tokio::test]
async fn a_manifest_the_realm_does_not_have_is_created_and_filled() {
    let keycloak = realm().await;
    // Nothing in the realm yet, then the group the create made.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .and(query_param("briefRepresentation", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&keycloak)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .and(query_param("exact", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "g1", "name": "city-leads", "attributes": { MANAGED_BY: [MANAGED_VALUE] } }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups/g1/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "u1", "email": "lead@hel.fi" }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{REALM}/users/.*/groups/.*$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;

    let mirror = mirror_with(vec![group("city-leads", &["lead@hel.fi"])]);
    let outcomes = sync(&keycloak).converge(&mirror).await;

    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert!(
        outcomes[0].drift.iter().any(|d| d.contains("was created")),
        "{outcomes:?}"
    );
    assert!(
        outcomes[0].drift.iter().any(|d| d.contains("was added")),
        "{outcomes:?}"
    );
    let wrote = wrote(&keycloak).await;
    assert!(wrote.contains(&format!("POST {REALM}/groups")), "{wrote:?}");
    assert!(
        wrote.contains(&format!("PUT {REALM}/users/u1/groups/g1")),
        "{wrote:?}"
    );

    // The drift is on the manifest, where the Access page reads it (PF-62).
    joinedcontext_portal::reconciler::groups::record(&mirror, &outcomes);
    let held = mirror
        .get(
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            "Group",
            "city-leads",
        )
        .expect("the group");
    let condition = &held.status.expect("status").conditions[0];
    assert_eq!(condition.r#type, "GroupSynced");
    assert_eq!(condition.reason.as_deref(), Some("DriftCorrected"));
}

#[tokio::test]
async fn a_console_edit_is_overwritten_and_reported_and_an_unknown_address_is_only_warned() {
    let keycloak = realm().await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "g1", "name": "city-leads", "attributes": { MANAGED_BY: [MANAGED_VALUE] } }
        ])))
        .mount(&keycloak)
        .await;
    // Somebody added themselves in the console; the manifest names two other people.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups/g1/members")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "u9", "email": "sneaky@hel.fi" }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users")))
        .and(query_param("email", "lead@hel.fi"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "u1", "email": "lead@hel.fi" }
        ])))
        .mount(&keycloak)
        .await;
    // Nobody has signed in under this address yet.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users")))
        .and(query_param("email", "newcomer@hel.fi"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&keycloak)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{REALM}/users/.*/groups/.*$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(format!("^{REALM}/users/.*/groups/.*$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;

    let mirror = mirror_with(vec![group(
        "city-leads",
        &["lead@hel.fi", "newcomer@hel.fi"],
    )]);
    let outcomes = sync(&keycloak).converge(&mirror).await;

    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert!(
        outcomes[0]
            .drift
            .iter()
            .any(|d| d.contains("sneaky@hel.fi") && d.contains("removed")),
        "the console edit is reported: {outcomes:?}"
    );
    assert!(
        outcomes[0]
            .warnings
            .iter()
            .any(|w| w.contains("newcomer@hel.fi")),
        "an address with no user yet is a warning: {outcomes:?}"
    );
    let wrote = wrote(&keycloak).await;
    assert!(
        wrote.contains(&format!("DELETE {REALM}/users/u9/groups/g1")),
        "{wrote:?}"
    );
    assert!(
        wrote.contains(&format!("PUT {REALM}/users/u1/groups/g1")),
        "{wrote:?}"
    );
}

#[tokio::test]
async fn a_group_without_the_attribute_is_never_written_and_never_removed() {
    let keycloak = realm().await;
    // The bootstrap administrators' group, made by hand, plus a managed one the repository
    // no longer declares.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "g0", "name": "platform-admins", "attributes": {} },
            { "id": "g7", "name": "old-team", "attributes": { MANAGED_BY: [MANAGED_VALUE] } }
        ])))
        .mount(&keycloak)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{REALM}/groups/g7")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;

    // The repository declares a group that shares its name with the unmanaged one.
    let mirror = mirror_with(vec![group("platform-admins", &[])]);
    let outcomes = sync(&keycloak).converge(&mirror).await;

    let clash = outcomes
        .iter()
        .find(|o| o.name == "platform-admins")
        .expect("the manifest");
    assert!(
        clash
            .error
            .as_deref()
            .is_some_and(|err| err.contains("never writes a group it does not manage")),
        "{outcomes:?}"
    );
    let removed = outcomes
        .iter()
        .find(|o| o.name == "old-team")
        .expect("the managed group nothing declares");
    assert!(removed.error.is_none(), "{outcomes:?}");

    let wrote = wrote(&keycloak).await;
    assert_eq!(
        wrote,
        vec![
            "POST /realms/bb/protocol/openid-connect/token".to_owned(),
            format!("DELETE {REALM}/groups/g7"),
        ],
        "only the managed group nothing declares was written"
    );
}

#[tokio::test]
async fn a_realm_that_refuses_the_client_fails_the_wave_and_writes_nothing() {
    let keycloak = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({ "error": "unauthorized" })))
        .mount(&keycloak)
        .await;

    let mirror = mirror_with(vec![group("city-leads", &["lead@hel.fi"])]);
    let outcomes = sync(&keycloak).converge(&mirror).await;

    assert_eq!(outcomes.len(), 1);
    assert!(
        outcomes[0]
            .error
            .as_deref()
            .is_some_and(|err| err.contains("refused the reconciler's client")),
        "{outcomes:?}"
    );
    assert!(wrote(&keycloak).await.len() == 1, "only the token request");
}

/// The secret never leaves the configuration: nothing this wave logs or reports carries it.
#[tokio::test]
async fn nothing_the_wave_answers_carries_the_client_secret() {
    let keycloak = realm().await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/groups")))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&keycloak)
        .await;
    let outcomes = sync(&keycloak).converge(&mirror_with(vec![])).await;
    let said: Value = json!(outcomes
        .iter()
        .map(|o| format!("{o:?}"))
        .collect::<Vec<_>>());
    assert!(!said.to_string().contains("s3cret"), "{said}");
}
