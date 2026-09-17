//! What a `Subscription` manifest does to the space it names (T-0931, CC-72, DS-16).
//!
//! The reconciler is the only wave that writes live broker state, so the tests are about what
//! leaves the Portal: the create, the update, the delete of a manifest that is gone, the refusal
//! of a reference that does not resolve — and that the credential is in the request and nowhere
//! else.

use std::sync::Arc;

use joinedcontext_portal::reconciler::subscriptions::{SubscriptionOutcome, SubscriptionSync};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const SPACE: &str = "air-quality";
const PROJECT: &str = "helsinki";
const DOMAIN: &str = "hel.fi";
const ID: &str = "urn:ngsi-ld:Subscription:hel.fi:air-quality:alerts";

/// The realm, answering the reconciler's client-credentials request.
async fn realm(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/realms/dev/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "the-reconcilers-token",
            "token_type": "Bearer",
            "expires_in": 300
        })))
        .mount(server)
        .await;
}

fn sync(server: &MockServer) -> SubscriptionSync {
    SubscriptionSync::new(
        server.uri(),
        &format!("{}/realms/dev", server.uri()),
        "portal-reconciler".to_owned(),
        "s3cr3t".to_owned(),
        DOMAIN,
    )
}

fn mirror_with(manifests: &[(&str, Value)]) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "ContextSpace",
        SPACE,
        json!({ "isSandbox": false }),
    ));
    for (name, spec) in manifests {
        mirror.upsert(envelope("Subscription", name, spec.clone()));
    }
    mirror
}

fn envelope(
    kind: &str,
    name: &str,
    spec: Value,
) -> joinedcontext_portal::resource::ResourceEnvelope {
    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": kind,
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": spec,
    }))
    .expect("an envelope")
}

/// The manifest of `Architecture/06`, without the credential.
fn declared() -> Value {
    json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": SPACE },
        "entities": [{ "type": "AirQualityObserved" }],
        "watchedAttributes": ["airQualityIndex"],
        "q": "airQualityIndex>50",
        "notification": {
            "endpoint": {
                "uri": "https://alerts.example.fi/hooks/air-quality",
                "accept": "application/json",
                "receiverInfo": [{ "key": "X-Space", "value": SPACE }]
            },
            "attributes": ["airQualityIndex", "location"],
            "format": "normalized"
        },
        "throttling": 60,
        "isActive": true
    })
}

/// Every request the space surface saw, as `(method, path, body)`.
async fn seen(server: &MockServer) -> Vec<(String, String, Value)> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r: &&Request| r.url.path().contains("/ngsi-ld/v1/subscriptions"))
        .map(|r| {
            (
                r.method.to_string(),
                r.url.path().to_owned(),
                serde_json::from_slice(&r.body).unwrap_or(Value::Null),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_declared_subscription_is_created_in_the_space_it_names() {
    let server = MockServer::start().await;
    realm(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SPACE}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/cs/{SPACE}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let mirror = mirror_with(&[("alerts", declared())]);
    let previous = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    assert_eq!(
        outcomes,
        vec![(
            PROJECT.to_owned(),
            "alerts".to_owned(),
            SubscriptionOutcome::Written
        )]
    );
    let posted = seen(&server)
        .await
        .into_iter()
        .find(|(verb, _, _)| verb == "POST")
        .expect("the space was asked to create it");
    assert_eq!(posted.2["id"], json!(ID), "the id is the manifest's");
    assert_eq!(posted.2["type"], json!("Subscription"));
    assert_eq!(posted.2["q"], json!("airQualityIndex>50"));
    assert_eq!(posted.2["throttling"], json!(60));
    assert_eq!(
        posted.2["notification"]["endpoint"]["uri"],
        json!("https://alerts.example.fi/hooks/air-quality")
    );
    assert_eq!(
        posted.2["notification"]["endpoint"]["receiverInfo"],
        json!([{ "X-Space": SPACE }]),
        "no credential is invented where the manifest declares none"
    );
}

#[tokio::test]
async fn a_subscription_the_space_already_holds_is_updated_not_duplicated() {
    let server = MockServer::start().await;
    realm(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SPACE}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "id": ID }])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/cs/{SPACE}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(409).set_body_string("already exists"))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(format!("/cs/{SPACE}/ngsi-ld/v1/subscriptions/{ID}")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let mirror = mirror_with(&[("alerts", declared())]);
    let previous = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    assert_eq!(outcomes[0].2, SubscriptionOutcome::Written);
    let patched = seen(&server)
        .await
        .into_iter()
        .find(|(verb, _, _)| verb == "PATCH")
        .expect("the one it already holds is patched");
    assert!(
        patched.2.get("id").is_none(),
        "a patch does not rename the subscription: {}",
        patched.2
    );
    assert_eq!(patched.2["q"], json!("airQualityIndex>50"));
    // Nothing was deleted: the id is the one the manifest declares.
    assert!(
        !seen(&server)
            .await
            .iter()
            .any(|(verb, _, _)| verb == "DELETE"),
        "a declared subscription must survive its own run"
    );
}

#[tokio::test]
async fn the_subscription_of_a_manifest_that_is_gone_is_removed() {
    let server = MockServer::start().await;
    realm(&server).await;
    // The space also holds one an application created through the API, in the very same URN
    // namespace: it was never declared here, so it is not this reconciler's to remove.
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SPACE}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": ID },
            { "id": "urn:ngsi-ld:Subscription:hel.fi:air-quality:written-by-an-application" },
        ])))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(r"^/cs/.+/ngsi-ld/v1/subscriptions/.+$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    // The run before declared `alerts`; this one declares nothing any more.
    let previous = mirror_with(&[("alerts", declared())]);
    let mirror = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;
    assert!(
        outcomes.is_empty(),
        "nothing was declared, so nothing was written"
    );

    let deleted: Vec<String> = seen(&server)
        .await
        .into_iter()
        .filter(|(verb, _, _)| verb == "DELETE")
        .map(|(_, path, _)| path)
        .collect();
    assert_eq!(
        deleted,
        vec![format!("/cs/{SPACE}/ngsi-ld/v1/subscriptions/{ID}")],
        "only what this reconciler wrote is removed; an application's subscription and another \
         organization's are left alone"
    );
}

#[tokio::test]
async fn a_credential_that_cannot_be_resolved_stops_the_subscription_before_any_request() {
    let server = MockServer::start().await;
    realm(&server).await;
    Mock::given(method("GET"))
        .and(path(format!("/cs/{SPACE}/ngsi-ld/v1/subscriptions")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let mut spec = declared();
    spec["notification"]["endpoint"]["secretRef"] =
        json!({ "name": "alerts-webhook", "key": "token" });
    let mirror = mirror_with(&[("alerts", spec)]);
    let previous = mirror_with(&[]);

    // No backend is configured, which is the honest shape of a Portal that holds no secrets.
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    let SubscriptionOutcome::Error(reason) = &outcomes[0].2 else {
        panic!("a subscription with an unresolvable credential must not be written: {outcomes:?}");
    };
    assert!(reason.contains("alerts-webhook"), "{reason}");
    assert!(
        !seen(&server)
            .await
            .iter()
            .any(|(verb, _, _)| verb == "POST"),
        "nothing may be posted for a subscription that has no credential"
    );
}

#[tokio::test]
async fn a_realm_that_refuses_the_client_writes_nothing_and_says_so_per_manifest() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/dev/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let mirror = mirror_with(&[("alerts", declared())]);
    let previous = mirror_with(&[]);
    let outcomes = sync(&server)
        .converge(
            &mirror,
            &previous,
            std::path::Path::new("/nonexistent"),
            None,
        )
        .await;

    let SubscriptionOutcome::Error(reason) = &outcomes[0].2 else {
        panic!("without a token nothing is written: {outcomes:?}");
    };
    assert!(
        reason.contains("refused the reconciler's client"),
        "{reason}"
    );
    assert!(
        seen(&server).await.is_empty(),
        "no space was touched at all"
    );
}
