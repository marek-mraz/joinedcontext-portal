//! Compiling `kind: App` into what runs it and what it may read (T-0227, AP-04, AP-05,
//! AP-25…AP-29, AP-13, AP-15, ADR-N-017).
//!
//! An app declares needs, never grants: `spec.dataNeeds` is the input, one `Endpoint` and one
//! `Policy` per need is the output, and the app reaches context data through nothing else
//! (AP-04). The pod-shaped half is the second half of the same idea: the app container listens
//! on the loopback address only and an oauth2-proxy sidecar is the pod's single entrance, so the
//! generated application carries no authentication code at all (AP-26, ADR-N-017).
//!
//! Rendering is a pure function of the manifest, the installation settings, the image CI built
//! and the reconciler-owned [`Credentials`]. Nothing here reaches a cluster: `render` returns the
//! objects, and applying them is the caller's business.

use std::collections::BTreeSet;
use std::fmt;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use jc_core::annotations::GENERATED_BY;
use jc_core::kinds::{
    AppClass, AppLifecycle, AppSpec, AppVisibility, DataNeed, EndpointSlug, Representation,
};
use jc_core::API_VERSION;
use jcctl::loader::{RawManifest, RawMetadata};
use serde_json::{json, Map, Value};

/// The tool the generated manifests name as their origin (MF-08).
pub const GENERATOR: &str = "portal/app-reconciler";

/// The port the oauth2-proxy sidecar listens on; the only port of the pod (AP-26).
///
/// `jcctl::apisix` already routes `/apps/{name}/*` to `app-{name}` on this port, so the Service
/// rendered here and the routing table rendered there have to agree on it.
pub const SIDECAR_PORT: u16 = 4180;

/// The port the app container serves on, bound to the loopback address (AP-26).
pub const APP_PORT: u16 = 8080;

/// The address the app container must bind. Not a suggestion: it is the reason the sidecar
/// cannot be bypassed (AP-26).
pub const APP_ADDRESS: &str = "127.0.0.1";

/// Base32 alphabet of RFC 4648 in the lowercase form [`EndpointSlug`] accepts (EP-02).
const SLUG_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// 26 characters of five bits each: 130 bits, over the 128 EP-02 asks for.
const SLUG_LEN: usize = 26;

/// Where the rendered objects run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The primary domain, `city.example.com`. Keycloak lives on `idm.{host}`.
    pub host: String,
    /// The Kubernetes namespace apps run in.
    pub namespace: String,
    /// The Keycloak realm of the organization; one confidential client per app lives in it.
    pub realm: String,
    /// The organization's domain, which becomes the policy assigner (`did:web:{domain}`).
    pub org_domain: String,
    /// The oauth2-proxy image, pinned by digest like every other image (AP-13).
    pub oauth2_proxy_image: String,
}

/// What the reconciler owns and Git never sees (AP-27, EP-02).
///
/// The two secrets configure the sidecar's OIDC client; the slug addresses the app's endpoint.
/// All three are unguessable and all three outlive a single render, so a second reconcile of an
/// unchanged app must pass the same values back rather than mint new ones — otherwise every run
/// would log every user out and move the endpoint.
#[derive(Clone)]
pub struct Credentials {
    client_secret: String,
    cookie_secret: String,
    endpoint_slug: EndpointSlug,
}

impl Credentials {
    /// Fresh credentials for an app that has none yet.
    pub fn generate() -> Self {
        let mut client_bytes = [0u8; 32];
        let mut cookie_bytes = [0u8; 32];
        let mut slug_bytes = [0u8; SLUG_LEN];
        OsRng.fill_bytes(&mut client_bytes);
        OsRng.fill_bytes(&mut cookie_bytes);
        OsRng.fill_bytes(&mut slug_bytes);

        // 32 is a divisor of 256, so masking five bits off a random byte draws from the
        // alphabet without bias and without a rejection loop.
        let slug: String = slug_bytes
            .iter()
            .map(|byte| char::from(SLUG_ALPHABET[usize::from(byte & 0x1f)]))
            .collect();

        Self {
            client_secret: URL_SAFE_NO_PAD.encode(client_bytes),
            // oauth2-proxy decodes the cookie secret and insists on 16, 24 or 32 raw bytes;
            // standard base64 of 32 bytes is what its own documentation tells operators to make.
            cookie_secret: STANDARD.encode(cookie_bytes),
            endpoint_slug: EndpointSlug::new(&slug).expect("the alphabet and length are EP-02's"),
        }
    }

    /// The credentials an app already has, as read back from the cluster and the repository.
    pub fn existing(
        client_secret: impl Into<String>,
        cookie_secret: impl Into<String>,
        endpoint_slug: &str,
    ) -> Result<Self, RenderError> {
        Ok(Self {
            client_secret: client_secret.into(),
            cookie_secret: cookie_secret.into(),
            endpoint_slug: EndpointSlug::new(endpoint_slug).map_err(RenderError::Slug)?,
        })
    }

    /// The endpoint slug, the only member that is not a secret.
    pub fn endpoint_slug(&self) -> &str {
        self.endpoint_slug.as_str()
    }
}

impl fmt::Debug for Credentials {
    /// Prints no secret. A rendered app ends up in logs and error reports; these do not.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("client_secret", &"redacted")
            .field("cookie_secret", &"redacted")
            .field("endpoint_slug", &self.endpoint_slug.as_str())
            .finish()
    }
}

/// The Kubernetes objects a pod-backed app needs; `static` apps have none (AP-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workload {
    /// App container plus oauth2-proxy sidecar (AP-26).
    pub deployment: Value,
    /// `app-{name}` on [`SIDECAR_PORT`], the name the APISIX upstream points at.
    pub service: Value,
    /// The client and cookie secrets of the sidecar's OIDC client (AP-27).
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
    /// The endpoint slug handed in is not a slug.
    #[error("the endpoint slug is not usable: {0}")]
    Slug(jc_core::Error),
}

/// Compiles one `App` manifest into its endpoint, its policies and, unless it is `static`, the
/// pod that serves it.
///
/// `image` is the digest-pinned reference CI built from `spec.source`; a `static` app ignores it.
pub fn render(
    manifest: &RawManifest,
    image: Option<&str>,
    credentials: &Credentials,
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
                name,
                project,
                &spec,
                image,
                credentials,
                settings,
            )?)
        }
    };

    Ok(Rendered {
        workload,
        endpoint: endpoint(name, project, space, &spec, credentials),
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
    credentials: &Credentials,
    settings: &Settings,
) -> Result<Workload, RenderError> {
    for reference in [image, settings.oauth2_proxy_image.as_str()] {
        if !pinned(reference) {
            return Err(RenderError::UnpinnedImage {
                image: reference.to_owned(),
            });
        }
    }

    let workload_name = format!("app-{name}");
    let labels = json!({
        "app.kubernetes.io/name": workload_name,
        "app.kubernetes.io/part-of": "joinedcontext",
        "app.kubernetes.io/managed-by": "joinedcontext-portal",
        "joinedcontext.com/app": name,
        "joinedcontext.com/project": project,
    });
    let selector = json!({ "app.kubernetes.io/name": workload_name });
    let secret_name = format!("{workload_name}-oauth2");

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
                    "containers": [
                        app_container(name, image, spec, credentials, settings),
                        sidecar_container(name, project, spec, &secret_name, settings),
                    ],
                    "volumes": [
                        { "name": "tmp-app", "emptyDir": {} },
                        { "name": "tmp-sidecar", "emptyDir": {} },
                    ],
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
                "port": SIDECAR_PORT,
                "targetPort": SIDECAR_PORT,
                "protocol": "TCP",
            }],
        },
    });

    let secret = json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": object_meta(&secret_name, settings, &labels),
        "type": "Opaque",
        "stringData": {
            "client-secret": credentials.client_secret,
            "cookie-secret": credentials.cookie_secret,
        },
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

/// The app itself: bound to the loopback address, with no port published and no probe.
///
/// A container the kubelet cannot reach cannot be probed by the kubelet either — `httpGet` opens
/// the connection to the pod address, which this container deliberately does not answer on. The
/// sidecar's readiness is what gates traffic, and the sidecar cannot be ready without its
/// upstream. That is the price of AP-26 and it is the right way round.
fn app_container(
    name: &str,
    image: &str,
    spec: &AppSpec,
    credentials: &Credentials,
    settings: &Settings,
) -> Value {
    let mut env = vec![
        json!({ "name": "JC_BIND_ADDRESS", "value": format!("{APP_ADDRESS}:{APP_PORT}") }),
        json!({ "name": "JC_BASE_PATH", "value": format!("/apps/{name}/") }),
        json!({
            "name": "JC_ENDPOINT_URL",
            "value": format!("https://{}/api/endpoint/{}/", settings.host, credentials.endpoint_slug()),
        }),
    ];
    if spec.visibility == AppVisibility::Public {
        // A public app is called by people who never logged in, so the backend has to know that
        // an absent `X-Forwarded-Access-Token` is normal rather than a bug (AP-28).
        env.push(json!({ "name": "JC_ANONYMOUS", "value": "true" }));
    }

    json!({
        "name": "app",
        "image": image,
        "imagePullPolicy": "IfNotPresent",
        "env": env,
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

/// The login front: the only container of the pod that listens on the pod address (AP-26).
fn sidecar_container(
    name: &str,
    project: &str,
    spec: &AppSpec,
    secret_name: &str,
    settings: &Settings,
) -> Value {
    let prefix = format!("/apps/{name}");
    let mut args = vec![
        "--provider=keycloak-oidc".to_owned(),
        format!(
            "--oidc-issuer-url=https://idm.{}/realms/{}",
            settings.host, settings.realm
        ),
        format!("--client-id=app-{name}"),
        format!(
            "--redirect-url=https://{}{prefix}/oauth2/callback",
            settings.host
        ),
        format!("--http-address=0.0.0.0:{SIDECAR_PORT}"),
        format!("--upstream=http://{APP_ADDRESS}:{APP_PORT}/"),
        format!("--proxy-prefix={prefix}/oauth2"),
        format!("--cookie-name=_oauth2_proxy_{name}"),
        format!("--cookie-path={prefix}/"),
        "--cookie-secure=true".to_owned(),
        "--cookie-httponly=true".to_owned(),
        "--cookie-samesite=lax".to_owned(),
        // A cookie session cannot be revoked from Keycloak, so it is kept short and re-checked
        // against the provider every quarter of an hour instead: a back-channel logout ends the
        // app session within that window rather than instantly (AP-29).
        "--cookie-expire=1h".to_owned(),
        "--cookie-refresh=15m".to_owned(),
        "--code-challenge-method=S256".to_owned(),
        // APISIX terminates TLS and is the only client of this port; without this the proxy
        // builds its redirect from the pod address and the login leaves the platform host.
        "--reverse-proxy=true".to_owned(),
        "--skip-provider-button=true".to_owned(),
        "--email-domain=*".to_owned(),
        "--pass-basic-auth=false".to_owned(),
        // What AP-28 is about: the app calls its endpoint with the caller's own token, so the
        // grant is the intersection of the user and the endpoint (GW10).
        "--pass-access-token=true".to_owned(),
        "--pass-user-headers=true".to_owned(),
        "--silence-ping-logging=true".to_owned(),
    ];
    match spec.visibility {
        // Anonymous callers pass through; the login route stays reachable for those who want it
        // and the endpoint enforces the public grant anyway (AP-28, GW22).
        AppVisibility::Public => args.push("--skip-auth-route=^/".to_owned()),
        // The realm is the organization, so membership of it is the whole check.
        AppVisibility::Organization => {}
        AppVisibility::Project | AppVisibility::Private => {
            args.push(format!("--allowed-groups=/{project}"));
        }
    }

    json!({
        "name": "oauth2-proxy",
        "image": settings.oauth2_proxy_image,
        "imagePullPolicy": "IfNotPresent",
        "args": args,
        "env": [
            {
                "name": "OAUTH2_PROXY_CLIENT_SECRET",
                "valueFrom": { "secretKeyRef": { "name": secret_name, "key": "client-secret" } },
            },
            {
                "name": "OAUTH2_PROXY_COOKIE_SECRET",
                "valueFrom": { "secretKeyRef": { "name": secret_name, "key": "cookie-secret" } },
            },
        ],
        "ports": [{ "name": "http", "containerPort": SIDECAR_PORT, "protocol": "TCP" }],
        "readinessProbe": {
            "httpGet": { "path": "/ready", "port": SIDECAR_PORT },
            "periodSeconds": 5,
            "timeoutSeconds": 3,
        },
        "livenessProbe": {
            "httpGet": { "path": "/ping", "port": SIDECAR_PORT },
            "periodSeconds": 15,
            "timeoutSeconds": 3,
        },
        "securityContext": {
            "readOnlyRootFilesystem": true,
            "allowPrivilegeEscalation": false,
            "capabilities": { "drop": ["ALL"] },
        },
        "resources": {
            "requests": { "cpu": "10m", "memory": "32Mi" },
            "limits": { "cpu": "200m", "memory": "128Mi" },
        },
        "volumeMounts": [{ "name": "tmp-sidecar", "mountPath": "/tmp" }],
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
            // Only the gateway may open a connection into the pod, and only on the sidecar's
            // port: the app container has no reachable port at all (AP-26).
            "ingress": [{
                "from": [{
                    "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "apisix" } },
                    "podSelector": { "matchLabels": { "app.kubernetes.io/name": "apisix" } },
                }],
                "ports": [{ "protocol": "TCP", "port": SIDECAR_PORT }],
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
                // Both calls an app pod makes — the sidecar's OIDC discovery and the backend's
                // own endpoint — carry the public host in the URL, so both are DNATed to the
                // ingress controller before this rule is evaluated and neither can be written as
                // a pod selector. The same trap the Portal's own policy documents; 443 and the
                // controller's container port are what the rule can narrow to.
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
    credentials: &Credentials,
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
        "slug": credentials.endpoint_slug(),
        "audience": audience,
        "enabledRepresentations": representations,
    });
    if !allowed_projects.is_empty() {
        endpoint_spec["allowedProjects"] = json!(allowed_projects);
    }
    if let Some(rate) = spec.limits.as_ref().and_then(|l| l.requests_per_minute) {
        endpoint_spec["rateLimits"] = json!({ "requestsPerMinute": rate });
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
