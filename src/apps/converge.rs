//! Putting what [`super::reconciler::render`] compiles onto the cluster (T-0411, AP-13,
//! AP-13a, AP-18, AP-21, AP-27).
//!
//! Rendering is a pure function; this is the half with a side effect. One App manifest becomes
//! four server-side applied objects, a retired one becomes four deletions, and everything else
//! is a documented skip rather than a silent nothing.
//!
//! Two properties make a second run cheap and safe. Server-side apply is idempotent by
//! construction, so an unchanged app converges to no change at the API server. And the three
//! values the reconciler owns and Git never sees — the two OIDC secrets and the endpoint slug —
//! are read back from the Secret before rendering, so a second run does not log every user out
//! and move the app's endpoint (AP-27, EP-02).
//!
//! One app's failure never stops another's: the loop reports per app and keeps going, because a
//! single broken manifest should not freeze every other app on the cluster.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use jc_core::kinds::{AppLifecycle, AppSpec};
use jcctl::loader::{RawManifest, Repository};
use serde_json::Value;

use super::kube::{KubeClient, KubeError};
use super::reconciler::{render, Credentials, RenderError, Settings};

/// The digest the build lane writes back when it publishes the image (AP-13a).
pub const IMAGE_ANNOTATION: &str = "joinedcontext.com/image";

/// The four objects an app owns, as the client addresses them.
const OBJECTS: [(&str, &str); 4] = [
    ("apps/v1", "Deployment"),
    ("v1", "Service"),
    ("networking.k8s.io/v1", "NetworkPolicy"),
    ("v1", "Secret"),
];

/// What one App did on one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The four objects now match the manifest.
    Applied,
    /// The app is retired and its objects are gone (AP-21).
    Deleted,
    /// Nothing was attempted, and this is why.
    Skipped(String),
}

/// Why one app could not be converged.
#[derive(Debug, thiserror::Error)]
pub enum ConvergeError {
    /// The manifest does not compile into objects.
    #[error("{0}")]
    Render(#[from] RenderError),
    /// The API server refused or could not be reached.
    #[error("{0}")]
    Kube(#[from] KubeError),
    /// The Secret exists and is not the one this reconciler wrote.
    #[error(
        "the secret app-{name}-oauth2 exists without the key {key}, so it is not this reconciler's"
    )]
    ForeignSecret {
        /// The app whose secret it should have been.
        name: String,
        /// The key that is missing.
        key: &'static str,
    },
}

/// Applies the objects of every App in the repository.
pub struct Converger {
    kube: KubeClient,
    settings: Settings,
}

impl Converger {
    /// A converger for one installation's apps namespace.
    pub fn new(kube: KubeClient, settings: Settings) -> Self {
        Self { kube, settings }
    }

    /// Converges every App in the repository, in the loader's deterministic order.
    ///
    /// Returns one line per app, so the caller logs what happened without deciding what an
    /// outcome means.
    pub async fn converge(&self, repository: &Repository) -> Vec<(String, ConvergeResult)> {
        let mut report = Vec::new();
        for (id, resource) in repository.iter() {
            if resource.manifest.kind != "App" {
                continue;
            }
            let outcome = self.converge_one(&resource.manifest).await;
            report.push((id.to_string(), outcome));
        }
        report
    }

    /// Converges one App manifest.
    pub async fn converge_one(&self, manifest: &RawManifest) -> ConvergeResult {
        let name = manifest.metadata.name.clone();
        let spec: AppSpec = match serde_json::from_value(manifest.spec.clone()) {
            Ok(spec) => spec,
            Err(err) => {
                return Err(ConvergeError::Render(RenderError::Spec(
                    jc_core::Error::Parse(err.to_string()),
                )))
            }
        };

        // A retired app is the one lifecycle that acts without rendering: there is nothing to
        // compile, only four objects to remove (AP-21).
        if spec.lifecycle == AppLifecycle::Retired {
            self.delete_objects(&name).await?;
            return Ok(Outcome::Deleted);
        }
        if !matches!(
            spec.lifecycle,
            AppLifecycle::Preview | AppLifecycle::Published
        ) {
            return Ok(Outcome::Skipped(format!(
                "an app in state {} renders no pod (AP-18)",
                spec.lifecycle.as_str()
            )));
        }

        // A static app is served by the Portal's own static host, so it has no objects at all
        // and its absence here is the design, not a gap (AP-14).
        let image = match annotation(manifest, IMAGE_ANNOTATION) {
            Some(image) => image,
            None if spec.class == jc_core::kinds::AppClass::Static => {
                return Ok(Outcome::Skipped(
                    "a static app is served by the Portal, not by a pod (AP-14)".to_owned(),
                ))
            }
            None => {
                return Ok(Outcome::Skipped(format!(
                    "no {IMAGE_ANNOTATION} yet, so the build has not published an image (AP-13a)"
                )))
            }
        };

        let credentials = self.credentials_of(&name).await?;
        let rendered = render(manifest, Some(&image), &credentials, &self.settings)?;
        let Some(workload) = rendered.workload else {
            return Ok(Outcome::Skipped(
                "a static app is served by the Portal, not by a pod (AP-14)".to_owned(),
            ));
        };

        // The Secret first: the Deployment references it, and a pod that starts before its
        // secret exists is a pod in CreateContainerConfigError until the next run.
        for object in [
            &workload.secret,
            &workload.network_policy,
            &workload.service,
            &workload.deployment,
        ] {
            self.kube.apply(object).await?;
        }
        Ok(Outcome::Applied)
    }

    /// The credentials this app already has, or fresh ones the first time it is deployed.
    async fn credentials_of(&self, name: &str) -> Result<Credentials, ConvergeError> {
        let secret_name = format!("app-{name}-oauth2");
        let Some(secret) = self
            .kube
            .get("v1", "Secret", &self.settings.namespace, &secret_name)
            .await?
        else {
            return Ok(Credentials::generate());
        };

        let read = |key: &'static str| -> Result<String, ConvergeError> {
            secret_value(&secret, key).ok_or(ConvergeError::ForeignSecret {
                name: name.to_owned(),
                key,
            })
        };
        Credentials::existing(
            read("client-secret")?,
            read("cookie-secret")?,
            &read("endpoint-slug")?,
        )
        .map_err(ConvergeError::Render)
    }

    /// Removes the four objects of one app; removing what is not there succeeds (CC-18).
    async fn delete_objects(&self, name: &str) -> Result<(), ConvergeError> {
        for (api_version, kind) in OBJECTS {
            let object_name = match kind {
                "Secret" => format!("app-{name}-oauth2"),
                _ => format!("app-{name}"),
            };
            self.kube
                .delete(api_version, kind, &self.settings.namespace, &object_name)
                .await?;
        }
        Ok(())
    }
}

/// What one app's convergence produced.
pub type ConvergeResult = Result<Outcome, ConvergeError>;

/// One annotation of a manifest, as a `String` because `RawMetadata` keeps the rest untyped.
fn annotation(manifest: &RawManifest, key: &str) -> Option<String> {
    manifest
        .metadata
        .rest
        .get("annotations")?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

/// One value of a Secret as the API server returns it: base64 in `data`, plain in `stringData`.
///
/// `stringData` is write-only in Kubernetes and never comes back, but a test double and a
/// hand-written fixture both use it, and accepting either costs one branch.
fn secret_value(secret: &Value, key: &str) -> Option<String> {
    if let Some(plain) = secret.get("stringData").and_then(|data| data.get(key)) {
        return plain.as_str().map(str::to_owned);
    }
    let encoded = secret.get("data")?.get(key)?.as_str()?;
    let bytes = STANDARD.decode(encoded).ok()?;
    String::from_utf8(bytes).ok()
}
