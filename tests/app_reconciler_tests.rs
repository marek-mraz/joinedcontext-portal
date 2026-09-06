//! What an `App` manifest compiles into (T-0227, AP-04, AP-05, AP-13, AP-18, AP-25…AP-28).
//!
//! Two properties carry the security of this kind and both are asserted here: the app container
//! has no way in except through the oauth2-proxy sidecar, and the rendered grants say exactly
//! what the app declared it needs and nothing more.

use jc_core::kinds::{EndpointSpec, PolicySpec};
use jcctl::loader::RawManifest;
use joinedcontext_portal::apps::reconciler::{
    render, Credentials, RenderError, Settings, APP_PORT, SIDECAR_PORT,
};
use serde_json::{json, Value};

const APP_IMAGE: &str =
    "ghcr.io/bb/apps/air-quality@sha256:1111111111111111111111111111111111111111111111111111111111111111";
const PROXY_IMAGE: &str =
    "quay.io/oauth2-proxy/oauth2-proxy@sha256:2222222222222222222222222222222222222222222222222222222222222222";

fn settings() -> Settings {
    Settings {
        host: "bb.example.com".into(),
        namespace: "joinedcontext".into(),
        realm: "banskabystrica".into(),
        org_domain: "banskabystrica.sk".into(),
        oauth2_proxy_image: PROXY_IMAGE.into(),
    }
}

/// The manifest of the architecture chapter's own example, trimmed to what rendering reads.
fn app(overrides: Value) -> RawManifest {
    let mut spec = json!({
        "kind": "fullstack",
        "source": { "path": "./src" },
        "build": { "rust": "1.90", "node": "22" },
        "visibility": "project",
        "lifecycle": "published",
        "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
            "types": ["AirQualityObserved", "District"],
            "attrs": ["pm10", "pm25", "location", "refDistrict"],
            "operations": ["queryEntity", "retrieveEntity"],
            "representations": ["ngsi-ld", "geojson"]
        }],
        "limits": { "requestsPerMinute": 600 }
    });
    for (key, value) in overrides.as_object().expect("an object of overrides") {
        spec[key] = value.clone();
    }

    serde_json::from_value(json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "App",
        "metadata": { "name": "air-quality-today", "namespace": "ovzdusie" },
        "spec": spec,
    }))
    .expect("the fixture is a manifest")
}

fn containers(deployment: &Value) -> &Vec<Value> {
    deployment["spec"]["template"]["spec"]["containers"]
        .as_array()
        .expect("a pod has containers")
}

fn container<'a>(deployment: &'a Value, name: &str) -> &'a Value {
    containers(deployment)
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no container named {name}"))
}

fn args(container: &Value) -> Vec<String> {
    container["args"]
        .as_array()
        .expect("the sidecar is configured by flags")
        .iter()
        .map(|a| a.as_str().expect("a flag is a string").to_owned())
        .collect()
}

fn env(container: &Value, name: &str) -> Value {
    container["env"]
        .as_array()
        .expect("env")
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("no environment variable {name}"))
        .clone()
}

#[test]
fn the_pod_is_an_app_behind_an_oauth2_proxy_sidecar() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &Credentials::generate(),
        &settings(),
    )
    .expect("the app renders");
    let workload = rendered.workload.expect("a fullstack app runs in a pod");

    let names: Vec<&str> = containers(&workload.deployment)
        .iter()
        .map(|c| c["name"].as_str().expect("a container has a name"))
        .collect();
    assert_eq!(
        names,
        vec!["app", "oauth2-proxy"],
        "every service and fullstack pod carries the login front (AP-26)"
    );

    let sidecar = container(&workload.deployment, "oauth2-proxy");
    assert_eq!(sidecar["image"], PROXY_IMAGE);
    let flags = args(sidecar);
    assert!(flags.contains(&format!("--http-address=0.0.0.0:{SIDECAR_PORT}")));
    assert!(flags.contains(&format!("--upstream=http://127.0.0.1:{APP_PORT}/")));
    assert!(flags.contains(&"--provider=keycloak-oidc".to_string()));
    assert!(flags.contains(&"--client-id=app-air-quality-today".to_string()));
    assert!(flags.contains(
        &"--oidc-issuer-url=https://idm.bb.example.com/realms/banskabystrica".to_string()
    ));
    assert!(
        flags.contains(&"--pass-access-token=true".to_string()),
        "the app calls its endpoint with the caller's own token (AP-28)"
    );
    assert!(
        flags.contains(&"--cookie-path=/apps/air-quality-today/".to_string())
            && flags.contains(&"--cookie-secure=true".to_string())
            && flags.contains(&"--cookie-httponly=true".to_string())
            && flags.contains(&"--cookie-samesite=lax".to_string()),
        "the session cookie is scoped to the app and hardened (AP-29)"
    );
    assert!(
        flags.contains(
            &"--redirect-url=https://bb.example.com/apps/air-quality-today/oauth2/callback"
                .to_string()
        ),
        "the login returns to the platform host, not to the pod"
    );
}

#[test]
fn the_app_container_listens_on_the_loopback_address_only() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &Credentials::generate(),
        &settings(),
    )
    .expect("the app renders");
    let deployment = rendered.workload.expect("a pod").deployment;

    let app_container = container(&deployment, "app");
    assert_eq!(
        env(app_container, "JC_BIND_ADDRESS")["value"],
        format!("127.0.0.1:{APP_PORT}"),
        "the app binds the loopback address; the sidecar is the only entrance (AP-26)"
    );
    assert!(
        app_container.get("ports").is_none(),
        "the app publishes no container port at all (AP-26)"
    );

    let sidecar_ports = container(&deployment, "oauth2-proxy")["ports"]
        .as_array()
        .expect("the sidecar publishes its port")
        .clone();
    assert_eq!(sidecar_ports.len(), 1);
    assert_eq!(sidecar_ports[0]["containerPort"], SIDECAR_PORT);
}

#[test]
fn only_the_gateway_may_open_a_connection_into_the_pod() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &Credentials::generate(),
        &settings(),
    )
    .expect("the app renders");
    let policy = rendered.workload.expect("a pod").network_policy;

    let types = policy["spec"]["policyTypes"]
        .as_array()
        .expect("both directions");
    assert!(types.contains(&json!("Ingress")) && types.contains(&json!("Egress")));

    let ingress = policy["spec"]["ingress"].as_array().expect("one hole in");
    assert_eq!(ingress.len(), 1, "APISIX is the only source admitted");
    assert_eq!(
        ingress[0]["from"][0]["podSelector"]["matchLabels"]["app.kubernetes.io/name"],
        "apisix"
    );
    assert_eq!(ingress[0]["ports"][0]["port"], SIDECAR_PORT);
}

#[test]
fn every_data_need_becomes_one_policy_that_grants_no_more_than_it_asked() {
    let rendered = render(
        &app(json!({
            "dataNeeds": [
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["AirQualityObserved"],
                    "attrs": ["pm10", "refDistrict"],
                    "operations": ["queryEntity", "queryTemporal"],
                    "q": "pm10>=0",
                    "geoQ": { "within": { "scopeRef": "/geo/SK/BB" } },
                    "temporalQ": { "window": "P1D" },
                    "representations": ["ngsi-ld"]
                },
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["District"],
                    "operations": ["retrieveEntity"],
                    "representations": ["geojson"]
                }
            ]
        })),
        Some(APP_IMAGE),
        &Credentials::generate(),
        &settings(),
    )
    .expect("the app renders");

    assert_eq!(
        rendered.policies.len(),
        2,
        "one policy per data need (AP-05)"
    );

    let first = &rendered.policies[0];
    assert_eq!(first.kind, "Policy");
    assert_eq!(first.metadata.name, "app-air-quality-today-1");
    assert_eq!(first.metadata.namespace.as_deref(), Some("ovzdusie"));
    assert_eq!(first.spec["assigner"], "did:web:banskabystrica.sk");
    assert_eq!(
        first.spec["operations"],
        json!(["queryEntity", "queryTemporal"]),
        "the grant is the operations the need names, in its own order (R8)"
    );
    assert_eq!(
        first.spec["information"][0]["entities"],
        json!([{ "type": "AirQualityObserved" }])
    );
    assert_eq!(
        first.spec["information"][0]["propertyNames"],
        json!(["pm10", "refDistrict"]),
        "an attribute the app did not declare is not readable through this grant"
    );
    assert_eq!(first.spec["q"], "pm10>=0");
    assert_eq!(
        first.spec["scopeQ"], "/geo/SK/BB",
        "a geographic confinement is a scope narrowing (ADR-N-005)"
    );
    assert_eq!(first.spec["temporalQ"], "timerel=after;timeAt=P-1D");

    let second = &rendered.policies[1];
    assert_eq!(second.metadata.name, "app-air-quality-today-2");
    assert_eq!(second.spec["operations"], json!(["retrieveEntity"]));
    assert!(
        second.spec.get("q").is_none()
            && second.spec.get("scopeQ").is_none()
            && second.spec.get("temporalQ").is_none(),
        "a need without constraints renders none"
    );

    for policy in &rendered.policies {
        let parsed: PolicySpec =
            serde_json::from_value(policy.spec.clone()).expect("a rendered policy parses");
        parsed.validate().expect("a rendered policy is valid");
    }
}

#[test]
fn the_endpoint_is_the_union_of_what_the_needs_asked_for() {
    let credentials = Credentials::generate();
    let rendered = render(&app(json!({})), Some(APP_IMAGE), &credentials, &settings())
        .expect("the app renders");

    let endpoint = &rendered.endpoint;
    assert_eq!(endpoint.kind, "Endpoint");
    assert_eq!(
        endpoint.metadata.name, "app-air-quality-today",
        "the name the APISIX upstream and the app's own configuration both point at (AP-05)"
    );
    assert_eq!(endpoint.spec["slug"], credentials.endpoint_slug());
    assert_eq!(
        endpoint.spec["enabledRepresentations"],
        json!(["ngsi-ld", "geojson"]),
        "the union of the needs' representations (AP-05)"
    );
    assert_eq!(endpoint.spec["audience"], "project-list");
    assert_eq!(endpoint.spec["allowedProjects"], json!(["ovzdusie"]));
    assert_eq!(endpoint.spec["rateLimits"]["requestsPerMinute"], 600);

    let parsed: EndpointSpec =
        serde_json::from_value(endpoint.spec.clone()).expect("a rendered endpoint parses");
    parsed.validate().expect("a rendered endpoint is valid");
}

#[test]
fn a_public_app_passes_anonymous_callers_through_and_grants_the_public_role() {
    let rendered = render(
        &app(json!({ "visibility": "public" })),
        Some(APP_IMAGE),
        &Credentials::generate(),
        &settings(),
    )
    .expect("the app renders");

    let deployment = rendered.workload.expect("a pod").deployment;
    let flags = args(container(&deployment, "oauth2-proxy"));
    assert!(
        flags.iter().any(|f| f.starts_with("--skip-auth-route=")),
        "a public app answers callers who never logged in (AP-28)"
    );
    assert!(
        !flags.iter().any(|f| f.starts_with("--allowed-groups=")),
        "a public app admits everyone, so it restricts no group"
    );
    assert_eq!(
        env(container(&deployment, "app"), "JC_ANONYMOUS")["value"],
        "true"
    );

    assert_eq!(rendered.endpoint.spec["audience"], "public");
    assert_eq!(
        rendered.policies[0].spec["assignee"],
        json!({ "kind": "role", "id": "public" }),
        "anonymous callers act under the synthetic public role (GW22)"
    );
}

#[test]
fn a_service_app_is_granted_through_its_own_account() {
    let rendered = render(
        &app(json!({ "kind": "service" })),
        Some(APP_IMAGE),
        &Credentials::generate(),
        &settings(),
    )
    .expect("the app renders");

    assert_eq!(
        rendered.policies[0].spec["assignee"],
        json!({ "kind": "serviceAccount", "id": "app-air-quality-today" }),
        "a service app calls with its own account and nobody else's (AP-08)"
    );
    assert!(
        rendered.workload.is_some(),
        "a service app runs behind the same sidecar as a fullstack one (AP-26)"
    );
}

#[test]
fn the_two_secrets_are_reconciler_owned_and_reach_the_pod_by_reference() {
    let slug = Credentials::generate().endpoint_slug().to_owned();
    let credentials = Credentials::existing("client-secret-abc", "cookie-secret-xyz", &slug)
        .expect("the slug of an app that already has one");

    let rendered = render(&app(json!({})), Some(APP_IMAGE), &credentials, &settings())
        .expect("the app renders");
    let workload = rendered.workload.expect("a pod");

    assert_eq!(workload.secret["kind"], "Secret");
    assert_eq!(
        workload.secret["metadata"]["name"],
        "app-air-quality-today-oauth2"
    );
    assert_eq!(
        workload.secret["stringData"]["client-secret"],
        "client-secret-abc"
    );
    assert_eq!(
        workload.secret["stringData"]["cookie-secret"],
        "cookie-secret-xyz"
    );

    let deployment = workload.deployment.to_string();
    assert!(
        !deployment.contains("client-secret-abc") && !deployment.contains("cookie-secret-xyz"),
        "the Deployment names the Secret, it does not carry it (AP-27)"
    );
    let sidecar = container(&workload.deployment, "oauth2-proxy");
    assert_eq!(
        env(sidecar, "OAUTH2_PROXY_CLIENT_SECRET")["valueFrom"]["secretKeyRef"],
        json!({ "name": "app-air-quality-today-oauth2", "key": "client-secret" })
    );

    let printed = format!("{credentials:?}");
    assert!(
        !printed.contains("client-secret-abc") && !printed.contains("cookie-secret-xyz"),
        "a rendered app ends up in logs; its secrets do not"
    );
    assert!(printed.contains(&slug), "the slug is not a secret");
}

#[test]
fn a_static_app_gets_its_grants_and_no_pod() {
    let rendered = render(
        &app(json!({ "kind": "static", "visibility": "organization" })),
        None,
        &Credentials::generate(),
        &settings(),
    )
    .expect("a static app renders without an image");

    assert!(
        rendered.workload.is_none(),
        "a static bundle is served by the Portal, not by a Deployment (AP-14)"
    );
    assert_eq!(rendered.policies.len(), 1);
    assert_eq!(rendered.endpoint.spec["audience"], "organization");
}

#[test]
fn an_image_that_is_not_pinned_by_digest_is_refused() {
    let refused = render(
        &app(json!({})),
        Some("ghcr.io/bb/apps/air-quality:latest"),
        &Credentials::generate(),
        &settings(),
    )
    .expect_err("a tag can be moved under a running pod");
    assert!(
        matches!(refused, RenderError::UnpinnedImage { .. }),
        "got {refused:?}"
    );

    let mut loose = settings();
    loose.oauth2_proxy_image = "quay.io/oauth2-proxy/oauth2-proxy:v7.13.0".into();
    let refused = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &Credentials::generate(),
        &loose,
    )
    .expect_err("the sidecar image is pinned like every other (AP-13)");
    assert!(matches!(refused, RenderError::UnpinnedImage { .. }));
}

#[test]
fn an_app_that_is_not_deployed_yet_renders_nothing() {
    for state in ["draft", "retired"] {
        let refused = render(
            &app(json!({ "lifecycle": state })),
            Some(APP_IMAGE),
            &Credentials::generate(),
            &settings(),
        )
        .expect_err("only preview and published apps run (AP-18, AP-21)");
        assert!(
            matches!(refused, RenderError::NotDeployable { .. }),
            "{state}: got {refused:?}"
        );
    }
}

#[test]
fn needs_reaching_into_two_spaces_do_not_become_one_endpoint() {
    let refused = render(
        &app(json!({
            "dataNeeds": [
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["AirQualityObserved"],
                    "operations": ["queryEntity"]
                },
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "doprava" },
                    "types": ["TrafficFlowObserved"],
                    "operations": ["queryEntity"]
                }
            ]
        })),
        Some(APP_IMAGE),
        &Credentials::generate(),
        &settings(),
    )
    .expect_err("an app reads through exactly one endpoint (AP-04)");
    assert!(
        matches!(refused, RenderError::SeveralSpaces { .. }),
        "got {refused:?}"
    );
}

#[test]
fn a_manifest_with_nowhere_to_put_a_secret_stays_that_way() {
    // AP-16: the App kind refuses `secretRef` at parse time, and rendering never invents one.
    let refused = render(
        &serde_json::from_value(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "App",
            "metadata": { "name": "leaky", "namespace": "ovzdusie" },
            "spec": {
                "kind": "fullstack",
                "source": { "path": "./src" },
                "build": { "rust": "1.90" },
                "visibility": "project",
                "lifecycle": "published",
                "secretRef": { "name": "database" },
                "dataNeeds": [{
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                    "types": ["AirQualityObserved"],
                    "operations": ["queryEntity"]
                }]
            },
        }))
        .expect("a manifest envelope"),
        Some(APP_IMAGE),
        &Credentials::generate(),
        &settings(),
    )
    .expect_err("an app has nowhere to put a secret (AP-16)");
    assert!(matches!(refused, RenderError::Spec(_)), "got {refused:?}");
}

#[test]
fn a_generated_slug_is_unguessable_and_never_the_same_twice() {
    let first = Credentials::generate();
    let second = Credentials::generate();
    assert_ne!(first.endpoint_slug(), second.endpoint_slug());
    assert!(
        first.endpoint_slug().len() >= 26
            && first
                .endpoint_slug()
                .chars()
                .all(|c| matches!(c, 'a'..='z' | '2'..='7')),
        "the slug is base32 and carries at least 128 bits (EP-02)"
    );
    assert!(
        Credentials::existing("a", "b", "ovzdusie").is_err(),
        "a readable slug is not a slug (EP-02, EP-03)"
    );
}
