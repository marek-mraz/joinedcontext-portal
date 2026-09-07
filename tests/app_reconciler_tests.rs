//! What an `App` manifest compiles into (T-0227, T-0508, AP-04, AP-05, AP-13, AP-18, AP-25…AP-28,
//! ADR-N-019).
//!
//! Two properties carry the security of this kind and both are asserted here: the app container
//! is the pod's only container and has no way in except from the APISIX edge, which is the login
//! front; and the rendered grants say exactly what the app declared it needs and nothing more.

use jc_core::kinds::{EndpointSlug, EndpointSpec, PolicySpec};
use jcctl::loader::RawManifest;
use joinedcontext_portal::apps::reconciler::{
    generate_slug, render, RenderError, Settings, APP_LABEL, APP_PORT,
};
use serde_json::{json, Value};

const APP_IMAGE: &str =
    "ghcr.io/bb/apps/air-quality@sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn settings() -> Settings {
    Settings {
        host: "bb.example.com".into(),
        namespace: "joinedcontext".into(),
        org_domain: "banskabystrica.sk".into(),
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
        "limits": { "requestsPerMinute": 600, "maxFileRows": 20000 }
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
fn the_pod_is_the_app_container_alone_behind_the_edge() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
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
        vec!["app"],
        "no sidecar: the login front is the edge's openid-connect plugin (AP-26, ADR-N-019)"
    );
    let pod = &workload.deployment["spec"]["template"];
    assert_eq!(
        pod["metadata"]["labels"][APP_LABEL], "true",
        "the label the edge's own egress policy selects app pods by"
    );
    assert_eq!(
        pod["metadata"]["labels"]["app.kubernetes.io/name"],
        "app-air-quality-today"
    );
    assert_eq!(
        pod["spec"]["volumes"],
        json!([{ "name": "tmp-app", "emptyDir": {} }]),
        "one container, one scratch volume"
    );

    let app = container(&workload.deployment, "app");
    assert_eq!(app["image"], APP_IMAGE);
    assert_eq!(
        env(app, "JC_BIND_ADDRESS")["value"],
        format!("0.0.0.0:{APP_PORT}"),
        "the app listens on the pod address; APISIX opens the connection from another pod (AP-26)"
    );
    assert_eq!(
        env(app, "JC_BASE_PATH")["value"],
        "/apps/air-quality-today/"
    );
    assert_eq!(
        app["ports"],
        json!([{ "name": "http", "containerPort": APP_PORT, "protocol": "TCP" }]),
        "port 8080, named http, and no other"
    );
    assert_eq!(app["readinessProbe"]["httpGet"]["path"], "/healthz");
    assert_eq!(app["readinessProbe"]["httpGet"]["port"], "http");
    assert!(
        app.get("livenessProbe").is_none(),
        "readiness gates traffic; a liveness probe on a busy app would only restart it"
    );

    let dumped = workload.deployment.to_string();
    for stale in [
        "oauth2",
        "OAUTH2",
        "client-secret",
        "cookie-secret",
        "keycloak",
        "4180",
    ] {
        assert!(
            !dumped.contains(stale),
            "{stale} survived in the Deployment"
        );
    }
}

#[test]
fn the_service_publishes_the_app_port_the_edge_upstreams_to() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");
    let service = rendered.workload.expect("a pod").service;

    assert_eq!(service["metadata"]["name"], "app-air-quality-today");
    assert_eq!(service["spec"]["type"], "ClusterIP");
    assert_eq!(
        service["spec"]["selector"],
        json!({ "app.kubernetes.io/name": "app-air-quality-today" })
    );
    assert_eq!(
        service["spec"]["ports"],
        json!([{ "name": "http", "port": APP_PORT, "targetPort": "http", "protocol": "TCP" }]),
        "8080, the port jcctl's route table points app-{{name}} at (AP-26)"
    );
}

#[test]
fn only_the_gateway_may_open_a_connection_into_the_pod() {
    let rendered = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");
    let policy = rendered.workload.expect("a pod").network_policy;

    let types = policy["spec"]["policyTypes"]
        .as_array()
        .expect("both directions");
    assert!(types.contains(&json!("Ingress")) && types.contains(&json!("Egress")));
    assert_eq!(
        policy["spec"]["podSelector"],
        json!({ "matchLabels": { "app.kubernetes.io/name": "app-air-quality-today" } })
    );

    let ingress = policy["spec"]["ingress"].as_array().expect("one hole in");
    assert_eq!(ingress.len(), 1, "APISIX is the only source admitted");
    assert_eq!(
        ingress[0]["from"],
        json!([{
            "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "apisix" } },
            "podSelector": { "matchLabels": { "app.kubernetes.io/name": "apisix" } },
        }]),
        "the label the apisix component's own pods carry"
    );
    assert_eq!(
        ingress[0]["ports"],
        json!([{ "protocol": "TCP", "port": APP_PORT }]),
        "the app port, no other"
    );

    // Out: DNS, and the platform host for the endpoint (443 and the controller's port), which
    // is where both APISIX and Keycloak are reached from a pod.
    let egress = policy["spec"]["egress"].as_array().expect("two holes out");
    assert_eq!(egress.len(), 2);
    assert_eq!(
        egress[0]["to"][0]["podSelector"]["matchLabels"]["k8s-app"],
        "kube-dns"
    );
    assert_eq!(
        egress[1]["to"],
        json!([{ "ipBlock": { "cidr": "0.0.0.0/0" } }])
    );
    assert_eq!(
        egress[1]["ports"],
        json!([
            { "protocol": "TCP", "port": 443 },
            { "protocol": "TCP", "port": 8443 },
        ])
    );
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
        &generate_slug(),
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
    let slug = generate_slug();
    let rendered =
        render(&app(json!({})), Some(APP_IMAGE), &slug, &settings()).expect("the app renders");

    let endpoint = &rendered.endpoint;
    assert_eq!(endpoint.kind, "Endpoint");
    assert_eq!(
        endpoint.metadata.name, "app-air-quality-today",
        "the name the APISIX upstream and the app's own configuration both point at (AP-05)"
    );
    assert_eq!(endpoint.spec["slug"], slug.as_str());
    assert_eq!(
        endpoint.spec["enabledRepresentations"],
        json!(["ngsi-ld", "geojson"]),
        "the union of the needs' representations (AP-05)"
    );
    assert_eq!(endpoint.spec["audience"], "project-list");
    assert_eq!(endpoint.spec["allowedProjects"], json!(["ovzdusie"]));
    assert_eq!(endpoint.spec["rateLimits"]["requestsPerMinute"], 600);
    assert_eq!(
        endpoint.spec["fileLimits"]["maxFileRows"], 20000,
        "a download limit belongs to the app's endpoint, not to its author (AP-17)"
    );

    let parsed: EndpointSpec =
        serde_json::from_value(endpoint.spec.clone()).expect("a rendered endpoint parses");
    parsed.validate().expect("a rendered endpoint is valid");

    // The pod is told the same endpoint, and nothing else about the platform.
    let app = rendered.workload.expect("a pod").deployment;
    assert_eq!(
        env(container(&app, "app"), "JC_ENDPOINT_URL")["value"],
        format!("https://bb.example.com/api/endpoint/{}/", slug.as_str())
    );
}

#[test]
fn a_public_app_tells_its_container_that_anonymous_callers_are_normal() {
    let rendered = render(
        &app(json!({ "visibility": "public" })),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders");

    let deployment = rendered.workload.expect("a pod").deployment;
    assert_eq!(
        env(container(&deployment, "app"), "JC_ANONYMOUS")["value"],
        "true",
        "the edge passes anonymous requests through without X-Access-Token (AP-28)"
    );

    assert_eq!(rendered.endpoint.spec["audience"], "public");
    assert_eq!(
        rendered.policies[0].spec["assignee"],
        json!({ "kind": "role", "id": "public" }),
        "anonymous callers act under the synthetic public role (GW22)"
    );

    // A project app has no such flag: an absent token there is the edge's `unauth_action: auth`
    // never having let the request through, which is a bug worth seeing.
    let project = render(
        &app(json!({})),
        Some(APP_IMAGE),
        &generate_slug(),
        &settings(),
    )
    .expect("the app renders")
    .workload
    .expect("a pod")
    .deployment;
    assert!(container(&project, "app")["env"]
        .as_array()
        .expect("env")
        .iter()
        .all(|e| e["name"] != "JC_ANONYMOUS"));
}

#[test]
fn a_service_app_is_granted_through_its_own_account() {
    let rendered = render(
        &app(json!({ "kind": "service" })),
        Some(APP_IMAGE),
        &generate_slug(),
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
        "a service app runs in the same kind of pod as a fullstack one (AP-26)"
    );
}

#[test]
fn the_secret_holds_the_slug_alone_and_the_pod_does_not_reference_it() {
    let slug = EndpointSlug::new(generate_slug().as_str()).expect("a slug the app already has");

    let rendered =
        render(&app(json!({})), Some(APP_IMAGE), &slug, &settings()).expect("the app renders");
    let workload = rendered.workload.expect("a pod");

    assert_eq!(workload.secret["kind"], "Secret");
    assert_eq!(
        workload.secret["metadata"]["name"],
        "app-air-quality-today-endpoint"
    );
    assert_eq!(
        workload.secret["stringData"],
        json!({ "endpoint-slug": slug.as_str() }),
        "no client secret, no cookie secret: the reconciler owns no OIDC secret (AP-27)"
    );

    let deployment = workload.deployment.to_string();
    assert!(
        !deployment.contains("secretKeyRef") && !deployment.contains("secretName"),
        "the pod reads nothing from the Secret; the slug reaches it as JC_ENDPOINT_URL"
    );
}

#[test]
fn a_static_app_gets_its_grants_and_no_pod() {
    let rendered = render(
        &app(json!({ "kind": "static", "visibility": "organization" })),
        None,
        &generate_slug(),
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
        &generate_slug(),
        &settings(),
    )
    .expect_err("a tag can be moved under a running pod");
    assert!(
        matches!(refused, RenderError::UnpinnedImage { .. }),
        "got {refused:?}"
    );
}

#[test]
fn an_app_that_is_not_deployed_yet_renders_nothing() {
    for state in ["draft", "retired"] {
        let refused = render(
            &app(json!({ "lifecycle": state })),
            Some(APP_IMAGE),
            &generate_slug(),
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
        &generate_slug(),
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
        &generate_slug(),
        &settings(),
    )
    .expect_err("an app has nowhere to put a secret (AP-16)");
    assert!(matches!(refused, RenderError::Spec(_)), "got {refused:?}");
}

#[test]
fn a_generated_slug_is_unguessable_and_never_the_same_twice() {
    let first = generate_slug();
    let second = generate_slug();
    assert_ne!(first, second);
    assert!(
        first.as_str().len() >= 26
            && first
                .as_str()
                .chars()
                .all(|c| matches!(c, 'a'..='z' | '2'..='7')),
        "the slug is base32 and carries at least 128 bits (EP-02)"
    );
    assert!(
        EndpointSlug::new("ovzdusie").is_err(),
        "a readable slug is not a slug (EP-02, EP-03)"
    );
}
