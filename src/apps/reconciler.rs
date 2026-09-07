//! Compiling `kind: App` into what runs it and what it may read (T-0227, AP-04, AP-05,
//! AP-25…AP-29, AP-13, AP-15, ADR-N-019).
//!
//! An app declares needs, never grants: `spec.dataNeeds` is the input, one `Endpoint` and one
//! `Policy` per need is the output, and the app reaches context data through nothing else
//! (AP-04). The pod-shaped half is the second half of the same idea: the app container is the
//! pod's only container, reachable from the APISIX edge alone, and the edge's `openid-connect`
//! plugin is the login front, so the application carries no authentication code at all (AP-26,
//! ADR-N-019).
//!
//! Rendering is a pure function of the manifest, the installation settings, the image CI built
//! and the reconciler-owned endpoint slug — spelled out because `apps/mod.rs` also documents
//! `pub mod reconciler;` from the outside, and rustdoc resolves the two doc blocks merged, in
//! the parent scope, where a bare name here is not an item. Nothing here reaches a cluster:
//! `render` returns the objects, and applying them is the caller's business.

use std::collections::BTreeSet;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use jc_core::annotations::GENERATED_BY;
use jc_core::kinds::{
    AppClass, AppLifecycle, AppSpec, AppVisibility, DataNeed, EndpointSlug, Representation,
};
use jc_core::API_VERSION;
use jcctl::loader::{RawManifest, RawMetadata};
use serde_json::{json, Map, Value};

/// The tool the generated manifests name as their origin (MF-08).
pub const GENERATOR: &str = "portal/app-reconciler";

/// The port the app container serves on and the Service publishes; the only port of the pod
/// (AP-26).
///
/// `jcctl::apisix` already routes `/apps/{name}/*` to `app-{name}` on this port, so the Service
/// rendered here and the routing table rendered there have to agree on it.
pub const APP_PORT: u16 = 8080;

/// The address the app container must bind: every interface of the pod, because APISIX opens
/// the connection from another pod (AP-26). The NetworkPolicy is what keeps everyone else out.
pub const APP_ADDRESS: &str = "0.0.0.0";

/// The pod label the edge's own NetworkPolicy selects app pods by for its egress
/// (Deployment/10 §4): one value for every app, the name is in `app.kubernetes.io/name`.
pub const APP_LABEL: &str = "joinedcontext.com/app";

/// Base32 alphabet of RFC 4648 in the lowercase form [`EndpointSlug`] accepts (EP-02).
const SLUG_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// 26 characters of five bits each: 130 bits, over the 128 EP-02 asks for.
const SLUG_LEN: usize = 26;

/// Where the rendered objects run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The primary domain, `city.example.com`, which the app's endpoint is served on.
    pub host: String,
    /// The Kubernetes namespace apps run in.
    pub namespace: String,
    /// The organization's domain, which becomes the policy assigner (`did:web:{domain}`).
    pub org_domain: String,
}

/// A fresh endpoint slug for an app that has none yet (EP-02).
///
/// The slug is the one value the reconciler owns and Git never sees: it addresses the app's
/// endpoint, it is unguessable, and it outlives a single render, so a second reconcile of an
/// unchanged app must pass the same one back rather than mint a new one — otherwise every run
/// would move the endpoint under its own users. The Secret `app-{name}-endpoint` is where it is
/// kept between runs.
pub fn generate_slug() -> EndpointSlug {
    let mut slug_bytes = [0u8; SLUG_LEN];
    OsRng.fill_bytes(&mut slug_bytes);
    // 32 is a divisor of 256, so masking five bits off a random byte draws from the alphabet
    // without bias and without a rejection loop.
    let slug: String = slug_bytes
        .iter()
        .map(|byte| char::from(SLUG_ALPHABET[usize::from(byte & 0x1f)]))
        .collect();
    EndpointSlug::new(&slug).expect("the alphabet and length are EP-02's")
}

/// The Kubernetes objects a pod-backed app needs; `static` apps have none (AP-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workload {
    /// The app container alone, on [`APP_PORT`] (AP-26).
    pub deployment: Value,
    /// `app-{name}` on [`APP_PORT`], the name the APISIX upstream points at.
    pub service: Value,
    /// `app-{name}-endpoint`: the endpoint slug, kept between runs (EP-02).
    pub secret: Value,
    /// Default-deny in both directions, with the two exceptions the app cannot work without
    /// (AP-15).
    pub network_policy: Value,
}

/// Everything one `App` manifest compiles into.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The pod-shaped objects, absent for a `static` app.
    pub workload: Option<Workload>,
    /// `app-{name}`, the one endpoint the app may reach (AP-04).
    pub endpoint: RawManifest,
    /// One policy per data need, in the order the needs are declared (AP-05).
    pub policies: Vec<RawManifest>,
}

/// Why an app did not compile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// The manifest is not an `App`.
    #[error("expected kind App, got {kind}")]
    NotAnApp {
        /// The kind the manifest actually declared.
        kind: String,
    },
    /// The manifest is an App whose spec does not hold.
    #[error("the app spec is not valid: {0}")]
    Spec(#[from] jc_core::Error),
    /// A project-scoped kind without a project.
    #[error("an app carries the project it belongs to in metadata.namespace")]
    NoProject,
    /// Only `preview` and `published` apps run (AP-18, AP-21).
    #[error("an app in state {lifecycle} is not deployed")]
    NotDeployable {
        /// The state the manifest is in.
        lifecycle: String,
    },
    /// A pod-backed app needs the image CI built for it.
    #[error("a {class} app renders a Deployment and needs the image built from its source")]
    NoImage {
        /// The app class that asked for an image.
        class: String,
    },
    /// An image that is not pinned by digest (AP-13).
    #[error("{image} is not pinned by digest; a tag is mutable (AP-13)")]
    UnpinnedImage {
        /// The reference as written.
        image: String,
    },
    /// Data needs reaching into two spaces cannot become one endpoint (AP-04).
    #[error("an app reads through one endpoint, but its data needs name the spaces {first} and {second} (AP-04)")]
    SeveralSpaces {
        /// The space the first need names.
        first: String,
        /// The first space that differs from it.
        second: String,
    },
    /// The endpoint slug read back from the cluster is not a slug.
    #[error("the endpoint slug is not usable: {0}")]
    Slug(jc_core::Error),
}

/// Compiles one `App` manifest into its endpoint, its policies and, unless it is `static`, the
/// pod that serves it.
///
/// `image` is the digest-pinned reference CI built from `spec.source`; a `static` app ignores it.
/// `slug` is the app's endpoint slug, read back from the cluster or freshly generated.
pub fn render(
    manifest: &RawManifest,
    image: Option<&str>,
    slug: &EndpointSlug,
    settings: &Settings,
) -> Result<Rendered, RenderError> {
    if manifest.kind != "App" {
        return Err(RenderError::NotAnApp {
            kind: manifest.kind.clone(),
        });
    }
    let spec: AppSpec = serde_json::from_value(manifest.spec.clone())
        .map_err(|err| jc_core::Error::Parse(err.to_string()))?;
    spec.validate()?;

    let name = manifest.metadata.name.as_str();
    let project = manifest
        .metadata
        .namespace
        .as_deref()
        .ok_or(RenderError::NoProject)?;

    match spec.lifecycle {
        AppLifecycle::Preview | AppLifecycle::Published => {}
        state => {
            return Err(RenderError::NotDeployable {
                lifecycle: state.as_str().to_owned(),
            })
        }
    }

    let space = single_space(&spec)?;

    let workload = match spec.class {
        AppClass::Static => None,
        class => {
            let image = image.ok_or_else(|| RenderError::NoImage {
                class: class.to_string(),
            })?;
            Some(render_workload(
                name, project, &spec, image, slug, settings,
            )?)
        }
    };

    Ok(Rendered {
        workload,
        endpoint: endpoint(name, project, space, &spec, slug),
        policies: spec
            .data_needs
            .iter()
            .enumerate()
            .map(|(index, need)| policy(name, project, index, need, &spec, settings))
            .collect(),
    })
}

/// The one space every data need must name; two spaces cannot become one endpoint (AP-04).
fn single_space(spec: &AppSpec) -> Result<&str, RenderError> {
    let first = spec.data_needs[0].context_space_ref.name();
    for need in &spec.data_needs[1..] {
        let other = need.context_space_ref.name();
        if other != first {
            return Err(RenderError::SeveralSpaces {
                first: first.to_owned(),
                second: other.to_owned(),
            });
        }
    }
    Ok(first)
}

/// `repo@sha256:<64 hex>`; anything else is a tag, and a tag can be moved under the pod (AP-13).
fn pinned(image: &str) -> bool {
    match image.split_once("@sha256:") {
        Some((repository, digest)) => {
            !repository.is_empty()
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }
        None => false,
    }
}

fn render_workload(
    name: &str,
    project: &str,
    spec: &AppSpec,
    image: &str,
    slug: &EndpointSlug,
    settings: &Settings,
) -> Result<Workload, RenderError> {
    if !pinned(image) {
        return Err(RenderError::UnpinnedImage {
            image: image.to_owned(),
        });
    }

    let workload_name = format!("app-{name}");
    let labels = json!({
        "app.kubernetes.io/name": workload_name,
        "app.kubernetes.io/part-of": "joinedcontext",
        "app.kubernetes.io/managed-by": "joinedcontext-portal",
        APP_LABEL: "true",
        "joinedcontext.com/project": project,
    });
    let selector = json!({ "app.kubernetes.io/name": workload_name });
    let secret_name = format!("{workload_name}-endpoint");

    let deployment = json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": object_meta(&workload_name, settings, &labels),
        "spec": {
            "replicas": 1,
            "selector": { "matchLabels": selector },
            "template": {
                "metadata": {
                    "labels": labels,
                    "annotations": { "linkerd.io/inject": "enabled" },
                },
                "spec": {
                    // Nothing in the pod talks to the Kubernetes API, and a mounted token is
                    // the first thing a compromised app would use.
                    "automountServiceAccountToken": false,
                    "enableServiceLinks": false,
                    "securityContext": {
                        "runAsNonRoot": true,
                        "seccompProfile": { "type": "RuntimeDefault" },
                    },
                    "containers": [app_container(name, image, spec, slug, settings)],
                    "volumes": [{ "name": "tmp-app", "emptyDir": {} }],
                },
            },
        },
    });

    let service = json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": object_meta(&workload_name, settings, &labels),
        "spec": {
            "type": "ClusterIP",
            "selector": selector,
            "ports": [{
                "name": "http",
                "port": APP_PORT,
                "targetPort": "http",
                "protocol": "TCP",
            }],
        },
    });

    let secret = json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": object_meta(&secret_name, settings, &labels),
        "type": "Opaque",
        // The slug is here and not only in the rendered Endpoint because this Secret is what
        // the reconciler owns and reads back: the slug outlives a render, and one regenerated
        // on the next run would move the app's endpoint under its own users (EP-02). No client
        // secret and no cookie secret beside it: the login front is the edge's, not the
        // pod's (AP-27, ADR-N-019).
        "stringData": { "endpoint-slug": slug.as_str() },
    });

    Ok(Workload {
        deployment,
        service,
        network_policy: network_policy(&workload_name, settings, &labels, &selector),
        secret,
    })
}

fn object_meta(name: &str, settings: &Settings, labels: &Value) -> Value {
    json!({
        "name": name,
        "namespace": settings.namespace,
        "labels": labels,
        "annotations": { GENERATED_BY: GENERATOR },
    })
}

/// The app itself: the pod's only container, on [`APP_PORT`], with a readiness probe on
/// `/healthz` so no traffic reaches it before it can answer (Architecture/16 §5).
fn app_container(
    name: &str,
    image: &str,
    spec: &AppSpec,
    slug: &EndpointSlug,
    settings: &Settings,
) -> Value {
    let mut env = vec![
        json!({ "name": "JC_BIND_ADDRESS", "value": format!("{APP_ADDRESS}:{APP_PORT}") }),
        json!({ "name": "JC_BASE_PATH", "value": format!("/apps/{name}/") }),
        json!({
            "name": "JC_ENDPOINT_URL",
            "value": format!("https://{}/api/endpoint/{}/", settings.host, slug.as_str()),
        }),
    ];
    if spec.visibility == AppVisibility::Public {
        // A public app is called by people who never logged in, so the backend has to know that
        // an absent `X-Access-Token` is normal rather than a bug (AP-28).
        env.push(json!({ "name": "JC_ANONYMOUS", "value": "true" }));
    }

    json!({
        "name": "app",
        "image": image,
        "imagePullPolicy": "IfNotPresent",
        "env": env,
        "ports": [{ "name": "http", "containerPort": APP_PORT, "protocol": "TCP" }],
        "readinessProbe": {
            "httpGet": { "path": "/healthz", "port": "http" },
            "periodSeconds": 5,
            "timeoutSeconds": 3,
        },
        "securityContext": {
            "readOnlyRootFilesystem": true,
            "allowPrivilegeEscalation": false,
            "capabilities": { "drop": ["ALL"] },
        },
        "resources": {
            "requests": { "cpu": "50m", "memory": "128Mi" },
            "limits": { "cpu": "1", "memory": "512Mi" },
        },
        "volumeMounts": [{ "name": "tmp-app", "mountPath": "/tmp" }],
    })
}

/// Default-deny in both directions with two holes: APISIX in, and the platform host out
/// (AP-15, AP-26).
fn network_policy(name: &str, settings: &Settings, labels: &Value, selector: &Value) -> Value {
    json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": object_meta(name, settings, labels),
        "spec": {
            "podSelector": { "matchLabels": selector },
            "policyTypes": ["Ingress", "Egress"],
            // Only the gateway may open a connection into the pod, and only on the app's port:
            // the edge is the login front, so the port is not a door for anyone else (AP-26).
            "ingress": [{
                "from": [{
                    "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "apisix" } },
                    "podSelector": { "matchLabels": { "app.kubernetes.io/name": "apisix" } },
                }],
                "ports": [{ "protocol": "TCP", "port": APP_PORT }],
            }],
            "egress": [
                {
                    "to": [{
                        "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "kube-system" } },
                        "podSelector": { "matchLabels": { "k8s-app": "kube-dns" } },
                    }],
                    "ports": [
                        { "protocol": "UDP", "port": 53 },
                        { "protocol": "TCP", "port": 53 },
                    ],
                },
                // The one call an app pod makes, its own endpoint, carries the public host in
                // the URL, so it is DNATed to the ingress controller before this rule is
                // evaluated and cannot be written as a pod selector. The same trap the Portal's
                // own policy documents; 443 and the controller's container port are what the
                // rule can narrow to.
                {
                    "to": [{ "ipBlock": { "cidr": "0.0.0.0/0" } }],
                    "ports": [
                        { "protocol": "TCP", "port": 443 },
                        { "protocol": "TCP", "port": 8443 },
                    ],
                },
            ],
        },
    })
}

/// `app-{name}`: the one endpoint the app reads through (AP-04, AP-05).
fn endpoint(
    name: &str,
    project: &str,
    space: &str,
    spec: &AppSpec,
    slug: &EndpointSlug,
) -> RawManifest {
    let representations: Vec<&str> = {
        let declared: BTreeSet<Representation> = spec.representations();
        if declared.is_empty() {
            // An app that names no representation still needs one; NGSI-LD is the canonical
            // surface every other representation projects from (EP-05).
            vec![Representation::NgsiLd.as_str()]
        } else {
            declared.iter().map(Representation::as_str).collect()
        }
    };

    let (audience, allowed_projects) = match spec.visibility {
        AppVisibility::Public => ("public", Vec::new()),
        AppVisibility::Organization => ("organization", Vec::new()),
        // `private` has no audience of its own in the endpoint model; the owning project is the
        // narrowest one there is, and the app's policies bind who inside it may act (AP-18).
        AppVisibility::Project | AppVisibility::Private => ("project-list", vec![project]),
    };

    let mut endpoint_spec = json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": space },
        "slug": slug.as_str(),
        "audience": audience,
        "enabledRepresentations": representations,
    });
    if !allowed_projects.is_empty() {
        endpoint_spec["allowedProjects"] = json!(allowed_projects);
    }
    if let Some(rate) = spec.limits.as_ref().and_then(|l| l.requests_per_minute) {
        endpoint_spec["rateLimits"] = json!({ "requestsPerMinute": rate });
    }
    // Both halves of `spec.limits` are enforced on the app's own endpoint and nowhere else, so
    // an app's downloads never eat into what its author may pull elsewhere (AP-17, EP-44).
    if let Some(rows) = spec.limits.as_ref().and_then(|l| l.max_file_rows) {
        endpoint_spec["fileLimits"] = json!({ "maxFileRows": rows });
    }

    generated(format!("app-{name}"), project, "Endpoint", endpoint_spec)
}

/// One data need, compiled into the grant that carries it (AP-05).
fn policy(
    name: &str,
    project: &str,
    index: usize,
    need: &DataNeed,
    spec: &AppSpec,
    settings: &Settings,
) -> RawManifest {
    let mut information = json!({
        "entities": need
            .types
            .iter()
            .map(|entity_type| json!({ "type": entity_type }))
            .collect::<Vec<_>>(),
    });
    if !need.attrs.is_empty() {
        // ponytail: an app declares attributes as one flat list and nothing here knows which of
        // them are relationships, so both whitelists get the same names — a property name in
        // `relationshipNames` matches no relationship and widens nothing. Split them properly
        // when the space's DataModel is available to the reconciler.
        information["propertyNames"] = json!(need.attrs);
        information["relationshipNames"] = json!(need.attrs);
    }

    let mut policy_spec = json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": need.context_space_ref.name() },
        "assigner": format!("did:web:{}", settings.org_domain),
        "assignee": assignee(name, spec),
        "operations": need.operations,
        "information": [information],
    });
    if let Some(q) = &need.q {
        policy_spec["q"] = json!(q);
    }
    if let Some(scope_q) = scope_query(need) {
        policy_spec["scopeQ"] = json!(scope_q);
    }
    if let Some(temporal_q) = temporal_query(need) {
        policy_spec["temporalQ"] = json!(temporal_q);
    }

    // 1-based and in declaration order: stable across runs, so a second reconcile of an
    // unchanged app writes the same names and changes nothing.
    generated(
        format!("app-{name}-{}", index + 1),
        project,
        "Policy",
        policy_spec,
    )
}

/// Who the grant is made to (AP-07, AP-08, AP-28, GW22).
fn assignee(name: &str, spec: &AppSpec) -> Value {
    match (spec.visibility, spec.class) {
        // Anonymous callers act under the synthetic public role, so that is who is granted.
        (AppVisibility::Public, _) => json!({ "kind": "role", "id": "public" }),
        // A service app calls with its own account and nobody else's (AP-08).
        (_, AppClass::Service) => json!({ "kind": "serviceAccount", "id": format!("app-{name}") }),
        // Everyone else reaches the data with the caller's own token through the app's endpoint;
        // the grant belongs to the role the endpoint carries, and the effective permission is
        // still the intersection with what the caller holds (AP-07, AP-28, GW10).
        _ => json!({ "kind": "role", "id": format!("app-{name}") }),
    }
}

/// The scope narrowing of a need: its own scope query and its geographic confinement are both
/// scope strings, so two of them are one conjunction rather than a lost constraint (AP-05).
fn scope_query(need: &DataNeed) -> Option<String> {
    let within = need.geo_q.as_ref().map(|geo| geo.within.scope_ref.as_str());
    match (need.scope_q.as_deref(), within) {
        (Some(declared), Some(confined)) => Some(format!("({declared});({confined})")),
        (Some(only), None) | (None, Some(only)) => Some(only.to_owned()),
        (None, None) => None,
    }
}

/// `{ window: P1D }` becomes the temporal query the gateway already speaks (R7, EP-56).
fn temporal_query(need: &DataNeed) -> Option<String> {
    need.temporal_q.as_ref().map(|temporal| {
        let window = temporal
            .window
            .strip_prefix('P')
            .unwrap_or(&temporal.window);
        format!("timerel=after;timeAt=P-{window}")
    })
}

fn generated(name: String, project: &str, kind: &str, spec: Value) -> RawManifest {
    let mut rest = Map::new();
    rest.insert("annotations".to_owned(), json!({ GENERATED_BY: GENERATOR }));
    RawManifest {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: RawMetadata {
            name,
            namespace: Some(project.to_owned()),
            rest,
        },
        spec,
    }
}
