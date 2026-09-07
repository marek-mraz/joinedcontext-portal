//! Putting an App's objects on a cluster (T-0411, AP-13, AP-13a, AP-18, AP-21, AP-27).
//!
//! Every case runs the real converger against a stubbed API server, so what is asserted is the
//! request that actually leaves the Portal: which path, which verb, which body. The two
//! properties that matter cannot be seen from inside the process. A second run of an unchanged
//! app must not mint new secrets, and the Portal must not be able to address anything but the
//! four objects an app owns.

use jcctl::loader::RawManifest;
use joinedcontext_portal::apps::converge::{Converger, Outcome, IMAGE_ANNOTATION};
use joinedcontext_portal::apps::kube::{KubeClient, FIELD_MANAGER};
use joinedcontext_portal::apps::reconciler::Settings;
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const APP_IMAGE: &str =
    "ghcr.io/bb/apps/air-quality@sha256:1111111111111111111111111111111111111111111111111111111111111111";
const PROXY_IMAGE: &str =
    "quay.io/oauth2-proxy/oauth2-proxy@sha256:2222222222222222222222222222222222222222222222222222222222222222";
const NAMESPACE: &str = "joinedcontext";
const SLUG: &str = "abcdefghijklmnopqrstuvwxyz";
const TOKEN: &str = "the-projected-service-account-token-of-the-portal";

fn settings() -> Settings {
    Settings {
        host: "bb.example.com".into(),
        namespace: NAMESPACE.into(),
        realm: "banskabystrica".into(),
        org_domain: "banskabystrica.sk".into(),
        oauth2_proxy_image: PROXY_IMAGE.into(),
    }
}

/// A published full-stack app whose image the build lane has already written back.
fn app(lifecycle: &str, image: Option<&str>) -> RawManifest {
    let mut metadata = json!({ "name": "air-quality-today", "namespace": "ovzdusie" });
    if let Some(image) = image {
        metadata["annotations"] = json!({ IMAGE_ANNOTATION: image });
    }
    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "App",
        "metadata": metadata,
        "spec": {
            "kind": "fullstack",
            "source": { "path": "./src" },
            "build": { "rust": "1.90", "node": "22" },
            "visibility": "project",
            "lifecycle": lifecycle,
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                "types": ["AirQualityObserved"],
                "attrs": ["pm10", "location"],
                "operations": ["queryEntity"],
                "representations": ["ngsi-ld"]
            }]
        },
    }))
    .expect("the fixture is a manifest")
}

fn converger(api: &MockServer) -> Converger {
    Converger::new(
        KubeClient::with_token(&api.uri(), TOKEN).expect("a client"),
        settings(),
    )
}

/// The Secret an app that has already been deployed once has, as the API server returns it.
fn stored_secret() -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": { "name": "app-air-quality-today-oauth2", "namespace": NAMESPACE },
        "data": {
            "client-secret": "dGhlLWNsaWVudC1zZWNyZXQ=",
            "cookie-secret": "dGhlLWNvb2tpZS1zZWNyZXQtb2YtdGhpcnR5LXR3bw==",
            "endpoint-slug": "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXo=",
        },
    })
}

/// Answers 404 for the app's Secret: an app nobody has deployed yet.
async fn no_secret(api: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/namespaces/{NAMESPACE}/secrets/app-air-quality-today-oauth2"
        )))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "kind": "Status", "status": "Failure", "reason": "NotFound",
            "message": "secrets \"app-air-quality-today-oauth2\" not found",
        })))
        .mount(api)
        .await;
}

/// Accepts a server-side apply on one path and records it.
async fn accepts_apply(api: &MockServer, api_path: &str) {
    Mock::given(method("PATCH"))
        .and(path(api_path.to_owned()))
        .and(query_param("fieldManager", FIELD_MANAGER))
        .and(header("content-type", "application/apply-patch+yaml"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "Status" })))
        .mount(api)
        .await;
}

fn applied<'a>(requests: &'a [Request], api_path: &str) -> &'a Request {
    requests
        .iter()
        .find(|request| request.url.path() == api_path && request.method.as_str() == "PATCH")
        .unwrap_or_else(|| panic!("nothing was applied to {api_path}"))
}

fn body_of(request: &Request) -> Value {
    serde_json::from_slice(&request.body).expect("the body is the object")
}

const DEPLOYMENT: &str = "/apis/apps/v1/namespaces/joinedcontext/deployments/app-air-quality-today";
const SERVICE: &str = "/api/v1/namespaces/joinedcontext/services/app-air-quality-today";
const SECRET: &str = "/api/v1/namespaces/joinedcontext/secrets/app-air-quality-today-oauth2";
const POLICY: &str =
    "/apis/networking.k8s.io/v1/namespaces/joinedcontext/networkpolicies/app-air-quality-today";

#[tokio::test]
async fn a_published_app_becomes_its_four_objects_on_the_cluster() {
    let api = MockServer::start().await;
    no_secret(&api).await;
    for api_path in [DEPLOYMENT, SERVICE, SECRET, POLICY] {
        accepts_apply(&api, api_path).await;
    }

    let outcome = converger(&api)
        .converge_one(&app("published", Some(APP_IMAGE)))
        .await
        .expect("the app converges");
    assert_eq!(outcome, Outcome::Applied);

    let requests = api
        .received_requests()
        .await
        .expect("the stub recorded them");
    assert_eq!(
        body_of(applied(&requests, DEPLOYMENT))["kind"],
        json!("Deployment")
    );
    assert_eq!(
        body_of(applied(&requests, SERVICE))["kind"],
        json!("Service")
    );
    assert_eq!(
        body_of(applied(&requests, POLICY))["kind"],
        json!("NetworkPolicy")
    );

    // AP-13: the digest the annotation carries is the one that ends up in the pod.
    let deployment = body_of(applied(&requests, DEPLOYMENT));
    let images: Vec<String> = deployment["spec"]["template"]["spec"]["containers"]
        .as_array()
        .expect("containers")
        .iter()
        .map(|container| container["image"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(images.contains(&APP_IMAGE.to_owned()), "{images:?}");

    // The Secret is written before the Deployment that references it, or the pod starts
    // without its configuration and waits for the next run.
    let order: Vec<&str> = requests
        .iter()
        .filter(|request| request.method.as_str() == "PATCH")
        .map(|request| request.url.path())
        .collect();
    let secret_at = order.iter().position(|p| *p == SECRET).expect("the secret");
    let deployment_at = order
        .iter()
        .position(|p| *p == DEPLOYMENT)
        .expect("the deployment");
    assert!(secret_at < deployment_at, "{order:?}");
}

/// AP-27, EP-02: the run that follows a run must not log every user out and move the endpoint.
#[tokio::test]
async fn a_second_run_reuses_the_secrets_and_the_slug_the_app_already_has() {
    let api = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SECRET))
        .respond_with(ResponseTemplate::new(200).set_body_json(stored_secret()))
        .mount(&api)
        .await;
    for api_path in [DEPLOYMENT, SERVICE, SECRET, POLICY] {
        accepts_apply(&api, api_path).await;
    }

    converger(&api)
        .converge_one(&app("published", Some(APP_IMAGE)))
        .await
        .expect("the app converges");

    let requests = api
        .received_requests()
        .await
        .expect("the stub recorded them");
    let secret = body_of(applied(&requests, SECRET));
    assert_eq!(
        secret["stringData"]["client-secret"],
        json!("the-client-secret")
    );
    assert_eq!(
        secret["stringData"]["cookie-secret"],
        json!("the-cookie-secret-of-thirty-two")
    );
    assert_eq!(secret["stringData"]["endpoint-slug"], json!(SLUG));
}

/// AP-21: retiring an app removes what it was running, and nothing is rendered to do it.
#[tokio::test]
async fn a_retired_app_has_its_objects_deleted() {
    let api = MockServer::start().await;
    for api_path in [DEPLOYMENT, SERVICE, SECRET, POLICY] {
        Mock::given(method("DELETE"))
            .and(path(api_path.to_owned()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "Status" })))
            .mount(&api)
            .await;
    }

    let outcome = converger(&api)
        .converge_one(&app("retired", Some(APP_IMAGE)))
        .await
        .expect("the app converges");
    assert_eq!(outcome, Outcome::Deleted);

    let requests = api
        .received_requests()
        .await
        .expect("the stub recorded them");
    let deleted: Vec<&str> = requests
        .iter()
        .filter(|request| request.method.as_str() == "DELETE")
        .map(|request| request.url.path())
        .collect();
    for api_path in [DEPLOYMENT, SERVICE, SECRET, POLICY] {
        assert!(deleted.contains(&api_path), "{api_path} was not deleted");
    }
}

/// A retirement that failed halfway is re-run without a special case (CC-18).
#[tokio::test]
async fn deleting_an_object_that_is_already_gone_succeeds() {
    let api = MockServer::start().await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "kind": "Status", "reason": "NotFound", "message": "not found",
        })))
        .mount(&api)
        .await;

    let outcome = converger(&api)
        .converge_one(&app("retired", None))
        .await
        .expect("removing what is not there is not a failure");
    assert_eq!(outcome, Outcome::Deleted);
}

/// AP-18: a draft renders no pod, and the cluster is not called at all.
#[tokio::test]
async fn a_draft_app_deploys_nothing_and_asks_the_cluster_nothing() {
    let api = MockServer::start().await;

    let outcome = converger(&api)
        .converge_one(&app("draft", Some(APP_IMAGE)))
        .await
        .expect("a draft is not an error");
    assert!(matches!(outcome, Outcome::Skipped(_)), "{outcome:?}");
    assert!(api
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty()));
}

/// AP-13a: an app whose build has not published an image yet waits, rather than deploying
/// whatever ran last time.
#[tokio::test]
async fn an_app_without_an_image_annotation_waits_for_its_build() {
    let api = MockServer::start().await;

    let outcome = converger(&api)
        .converge_one(&app("published", None))
        .await
        .expect("a missing image is not an error");
    match outcome {
        Outcome::Skipped(reason) => assert!(reason.contains(IMAGE_ANNOTATION), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(api
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty()));
}

/// AP-13: the Portal addresses four kinds in one namespace. A request for anything else is
/// refused here, before RBAC is ever asked, so the two controls are independent.
#[tokio::test]
async fn the_portal_cannot_address_a_kind_an_app_does_not_own() {
    let api = MockServer::start().await;
    let client = KubeClient::with_token(&api.uri(), TOKEN).expect("a client");

    let refused = client
        .apply(&json!({
            "apiVersion": "rbac.authorization.k8s.io/v1",
            "kind": "ClusterRoleBinding",
            "metadata": { "name": "portal-admin", "namespace": NAMESPACE },
        }))
        .await;
    assert!(refused.is_err(), "a ClusterRoleBinding was addressable");
    assert!(api
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty()));
}

/// A name is a path segment, so a name that is not a DNS label never becomes one.
#[tokio::test]
async fn a_name_that_is_not_a_dns_label_never_becomes_a_path() {
    let api = MockServer::start().await;
    let client = KubeClient::with_token(&api.uri(), TOKEN).expect("a client");

    let refused = client
        .apply(&json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": { "name": "../../secrets/kubernetes-admin", "namespace": NAMESPACE },
        }))
        .await;
    assert!(refused.is_err(), "a traversal reached the API server");
    assert!(api
        .received_requests()
        .await
        .is_some_and(|requests| requests.is_empty()));
}
