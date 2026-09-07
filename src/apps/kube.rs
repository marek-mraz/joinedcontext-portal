//! The smallest Kubernetes client that can converge an app's objects (T-0411, AP-13, AP-18,
//! AP-21, AP-26).
//!
//! The Portal writes four kinds into one namespace and nothing else, so this is a hand-written
//! client over the `reqwest` the Portal already carries rather than a Kubernetes client
//! library: server-side apply is one `PATCH` with a content type, and a delete is a `DELETE`.
//! The dependency a library would add is larger than the code it would replace, and this way
//! the set of reachable objects is a `match` in one file.
//!
//! That match is a security control, not a convenience. A manifest that asked for a
//! `ClusterRoleBinding` is refused here, before the request is built, so the Portal's RBAC is
//! the second line of defence rather than the first (AP-13).
//!
//! The credential is a projected ServiceAccount token, which the kubelet rotates, so it is read
//! from disk per request instead of once at startup; a token cached for the lifetime of the pod
//! starts answering 401 after an hour.

use std::path::{Path, PathBuf};

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Certificate, StatusCode};
use serde_json::Value;
use url::Url;

/// Where the kubelet mounts the pod's ServiceAccount.
const SERVICE_ACCOUNT_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";

/// The in-cluster API server, whose certificate the mounted authority signs.
const IN_CLUSTER_API: &str = "https://kubernetes.default.svc";

/// The field manager every object the app reconciler owns is applied under (CC-18).
///
/// Server-side apply keeps one manager's fields separate from another's, so a field this
/// manager stops sending is removed, and a hand edit by a person shows up as a conflict that
/// `force` resolves in the reconciler's favour rather than as a silent divergence.
pub const FIELD_MANAGER: &str = "portal-app-reconciler";

/// Server-side apply's content type; the body is JSON, which is YAML.
const APPLY_PATCH: &str = "application/apply-patch+yaml";

/// The four kinds an App compiles into (AP-13, AP-15, AP-26, AP-27), and nothing else.
const KINDS: [(&str, &str, &str); 4] = [
    ("apps/v1", "Deployment", "deployments"),
    ("v1", "Service", "services"),
    ("v1", "Secret", "secrets"),
    ("networking.k8s.io/v1", "NetworkPolicy", "networkpolicies"),
];

/// Why a request to the API server did not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum KubeError {
    /// The object names a kind or an API group the Portal is not allowed to write.
    #[error("the Portal does not write {api_version} {kind}")]
    UnsupportedKind {
        /// The `apiVersion` of the object.
        api_version: String,
        /// The `kind` of the object.
        kind: String,
    },
    /// The object has no name, no namespace, or one that is not a DNS label.
    #[error("{field} is not a name the API server would accept: {value}")]
    NotAName {
        /// Which of the two names is wrong.
        field: &'static str,
        /// The value as written.
        value: String,
    },
    /// The API server answered, and refused.
    #[error("the API server answered {status} for {path}: {message}")]
    Api {
        /// The HTTP status.
        status: u16,
        /// The path that was called, which never carries a credential.
        path: String,
        /// The `message` of the Status object, or the body when it is not one.
        message: String,
    },
    /// The API server could not be reached.
    #[error("cannot reach the API server: {0}")]
    Transport(String),
    /// The ServiceAccount mount is not readable, so there is no identity to act as.
    #[error("cannot read {path}: {source}")]
    ServiceAccount {
        /// The file that could not be read.
        path: PathBuf,
        /// Why not.
        #[source]
        source: std::io::Error,
    },
    /// The base URL or the certificate authority is not usable.
    #[error("the API server client cannot be built: {0}")]
    Client(String),
}

/// Where the bearer token comes from.
#[derive(Debug, Clone)]
enum TokenSource {
    /// A projected ServiceAccount token, re-read per request because the kubelet rotates it.
    File(PathBuf),
    /// A fixed token, which is how the tests speak to a stub API server.
    Fixed(String),
}

/// A client for the four kinds the app reconciler owns, in one namespace at a time.
pub struct KubeClient {
    http: reqwest::Client,
    base: Url,
    token: TokenSource,
}

// The token never reaches a log, an error or a status page.
impl std::fmt::Debug for KubeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubeClient")
            .field("base", &self.base.as_str())
            .field("token", &"redacted")
            .finish()
    }
}

impl KubeClient {
    /// The client a pod builds from its own ServiceAccount, or `None` outside a cluster.
    ///
    /// A Portal running on a laptop has no token mount and no cluster to converge, and that is
    /// not an error: it serves the UI and the resource API and applies nothing.
    pub fn in_cluster() -> Result<Option<Self>, KubeError> {
        Self::from_mount(Path::new(SERVICE_ACCOUNT_DIR), IN_CLUSTER_API)
    }

    /// [`KubeClient::in_cluster`] against a mount and an API server named explicitly, which is
    /// what makes the in-cluster path testable at all.
    pub fn from_mount(dir: &Path, api: &str) -> Result<Option<Self>, KubeError> {
        let token = dir.join("token");
        if !token.is_file() {
            return Ok(None);
        }
        let authority = dir.join("ca.crt");
        let ca = match authority.is_file() {
            true => {
                Some(
                    std::fs::read(&authority).map_err(|source| KubeError::ServiceAccount {
                        path: authority,
                        source,
                    })?,
                )
            }
            false => None,
        };
        let base = Url::parse(api).map_err(|err| KubeError::Client(err.to_string()))?;
        Self::build(base, TokenSource::File(token), ca.as_deref()).map(Some)
    }

    /// A client against a named API server with a fixed token, for tests.
    pub fn with_token(api: &str, token: impl Into<String>) -> Result<Self, KubeError> {
        let base = Url::parse(api).map_err(|err| KubeError::Client(err.to_string()))?;
        Self::build(base, TokenSource::Fixed(token.into()), None)
    }

    fn build(base: Url, token: TokenSource, ca: Option<&[u8]>) -> Result<Self, KubeError> {
        let mut builder = reqwest::Client::builder();
        if let Some(pem) = ca {
            let certificate =
                Certificate::from_pem(pem).map_err(|err| KubeError::Client(err.to_string()))?;
            builder = builder.add_root_certificate(certificate);
        }
        let http = builder
            .build()
            .map_err(|err| KubeError::Client(err.to_string()))?;
        Ok(Self { http, base, token })
    }

    /// Applies one object, creating it or bringing it back to what the manifest says (CC-18).
    pub async fn apply(&self, object: &Value) -> Result<(), KubeError> {
        let path = path_of(
            object_kind(object)?,
            namespace_of(object)?,
            name_of(object)?,
        )?;
        let url = self.url(&format!("{path}?fieldManager={FIELD_MANAGER}&force=true"))?;
        let response = self
            .http
            .patch(url)
            .headers(self.headers(APPLY_PATCH)?)
            .body(object.to_string())
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        self.checked(response, path).await.map(|_| ())
    }

    /// Removes one object; removing what is not there succeeds, so a retirement is re-runnable.
    pub async fn delete(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> Result<(), KubeError> {
        let path = path_of(plural_of(api_version, kind)?, namespace, name)?;
        let response = self
            .http
            .delete(self.url(&path)?)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        self.checked(response, path).await.map(|_| ())
    }

    /// Reads one object, or `None` when it does not exist.
    pub async fn get(
        &self,
        api_version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> Result<Option<Value>, KubeError> {
        let path = path_of(plural_of(api_version, kind)?, namespace, name)?;
        let response = self
            .http
            .get(self.url(&path)?)
            .headers(self.headers("application/json")?)
            .send()
            .await
            .map_err(|err| KubeError::Transport(err.to_string()))?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = self.checked(response, path).await?;
        Ok(Some(body))
    }

    fn url(&self, path: &str) -> Result<Url, KubeError> {
        self.base
            .join(path)
            .map_err(|err| KubeError::Client(err.to_string()))
    }

    fn headers(&self, content_type: &str) -> Result<HeaderMap, KubeError> {
        let token = match &self.token {
            TokenSource::Fixed(token) => token.clone(),
            TokenSource::File(path) => std::fs::read_to_string(path)
                .map_err(|source| KubeError::ServiceAccount {
                    path: path.clone(),
                    source,
                })?
                .trim()
                .to_owned(),
        };
        let mut headers = HeaderMap::new();
        let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|err| KubeError::Client(err.to_string()))?;
        bearer.set_sensitive(true);
        headers.insert(AUTHORIZATION, bearer);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(content_type)
                .map_err(|err| KubeError::Client(err.to_string()))?,
        );
        Ok(headers)
    }

    async fn checked(&self, response: reqwest::Response, path: String) -> Result<Value, KubeError> {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status.is_success() {
            return Ok(serde_json::from_str(&body).unwrap_or(Value::Null));
        }
        // A Status object says why in one sentence; anything else is passed on as it came, and
        // neither carries the token, which is a header.
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|value| value.get("message")?.as_str().map(str::to_owned))
            .unwrap_or(body);
        Err(KubeError::Api {
            status: status.as_u16(),
            path,
            message,
        })
    }
}

/// The `apiVersion` and `kind` of an object, as the plural path segment they address.
fn object_kind(object: &Value) -> Result<&'static str, KubeError> {
    let api_version = object
        .get("apiVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    plural_of(api_version, kind)
}

/// The path segment one kind is addressed by, and the proof that the Portal may write it.
fn plural_of(api_version: &str, kind: &str) -> Result<&'static str, KubeError> {
    KINDS
        .iter()
        .find(|(group, name, _)| *group == api_version && *name == kind)
        .map(|(_, _, plural)| *plural)
        .ok_or_else(|| KubeError::UnsupportedKind {
            api_version: api_version.to_owned(),
            kind: kind.to_owned(),
        })
}

fn namespace_of(object: &Value) -> Result<&str, KubeError> {
    object
        .get("metadata")
        .and_then(|meta| meta.get("namespace"))
        .and_then(Value::as_str)
        .ok_or(KubeError::NotAName {
            field: "metadata.namespace",
            value: String::new(),
        })
}

fn name_of(object: &Value) -> Result<&str, KubeError> {
    object
        .get("metadata")
        .and_then(|meta| meta.get("name"))
        .and_then(Value::as_str)
        .ok_or(KubeError::NotAName {
            field: "metadata.name",
            value: String::new(),
        })
}

/// The API path of one object, with both names checked before they become path segments.
///
/// A name is a DNS label everywhere in Kubernetes, so checking it here costs nothing and closes
/// the only way a manifest could reach a path the Portal was not meant to call.
fn path_of(plural: &str, namespace: &str, name: &str) -> Result<String, KubeError> {
    for (field, value) in [("metadata.namespace", namespace), ("metadata.name", name)] {
        jc_core::names::validate_dns1123_label(value).map_err(|_| KubeError::NotAName {
            field,
            value: value.to_owned(),
        })?;
    }
    let group = KINDS
        .iter()
        .find(|(_, _, candidate)| *candidate == plural)
        .map(|(group, _, _)| *group)
        .unwrap_or("v1");
    let prefix = match group {
        "v1" => "/api/v1".to_owned(),
        group => format!("/apis/{group}"),
    };
    Ok(format!("{prefix}/namespaces/{namespace}/{plural}/{name}"))
}
