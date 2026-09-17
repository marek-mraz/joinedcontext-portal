//! Resolving a pipeline's `secretRef`s into the runner's environment (T-0927, PL-15…PL-17).
//!
//! A `bento.yaml` names a credential as `${MQTT_PASSWORD}` and nothing else (PL-16), so the
//! value has to be in the runner's own environment by the time the stream runs. Both secret
//! backends have been complete for a while and neither was reached: `jcctl apply` converges
//! through the `Platform` trait and speaks to Kubernetes nowhere, while this process already
//! stages the repository every sync and already writes the objects of an App. So the reconciler
//! resolves them here and writes them into one Secret the runner mounts.
//!
//! Three rules the shape follows, all of them from what goes wrong without them:
//!
//! * **One Secret per runner** (`pipeline-secrets`), because a runner is one pod with one
//!   environment, and a Secret per pipeline would be a mount the deployment cannot know about.
//! * **A clash is a refusal.** Two pipelines naming one `envVar` through different references
//!   would otherwise be served whichever value was written last, and the loser would read a
//!   credential belonging to the other pipeline.
//! * **A reference that does not resolve leaves the stream undeployed**, with the reason on the
//!   Pipeline. A stream that starts without the one credential it needs fails in the runner's
//!   log, where nobody is looking.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jc_core::envelope::SecretRef;
use jcctl::secrets::openbao::{self, BaoStore};
use jcctl::secrets::{identities_from_file, HttpBao, SecretStore};
use serde_json::{json, Value};

/// The Secret every runner mounts, whichever projects it serves.
pub const SECRET_NAME: &str = "pipeline-secrets";

/// Where the values come from. A deployment setting, never a manifest field (CC-06).
#[derive(Debug, Clone)]
pub enum Backend {
    /// The repository's own `*.sops.yaml`, decrypted with the age identity at this path. The
    /// tree is the one this sync staged, so a rotated file is read the run after it lands.
    Sops {
        /// `SOPS_AGE_KEY_FILE`: read per resolution rather than held, so the key spends no
        /// longer in this process's memory than a sync takes.
        age_key_file: PathBuf,
    },
    /// OpenBao's KV v2, reached with the pod's own ServiceAccount token (ADR-N-012).
    OpenBao {
        /// Base address, `https://openbao.openbao.svc:8200`.
        address: String,
        /// The Kubernetes auth role this Portal logs in as.
        role: String,
        /// Where the kubelet mounts the token the login presents.
        jwt_path: PathBuf,
    },
}

/// Why a pipeline's references could not become an environment. Every variant names the
/// reference and never the value (PL-17).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretError {
    /// The manifest names a reference and the deployment configured no backend at all.
    #[error("this Portal has no secret backend configured, so the reference to '{name}' cannot be resolved; set JC_PORTAL_SOPS_AGE_KEY_FILE or JC_PORTAL_OPENBAO_ADDR")]
    NoBackend {
        /// The secret the manifest names.
        name: String,
    },
    /// A reference carries no `envVar`, so there is nothing to interpolate it as.
    #[error("the reference to '{name}' names no envVar, and a runner reads a credential as an environment variable (PL-16)")]
    NoEnvVar {
        /// The secret the manifest names.
        name: String,
    },
    /// Two pipelines want one variable from different references.
    #[error("'{variable}' is claimed by {pipeline} and by {other} from different references; one runner has one environment, so rename one of them")]
    Clash {
        /// The environment variable both want.
        variable: String,
        /// The pipeline this run is resolving.
        pipeline: String,
        /// The pipeline that claimed it first.
        other: String,
    },
    /// The backend refused or does not hold it.
    #[error("the secret store has no '{name}': {reason}")]
    Unresolved {
        /// The secret the manifest names.
        name: String,
        /// What the backend said. Backend errors name paths and keys, never values.
        reason: String,
    },
}

/// One pipeline's references, resolved.
type Environment = BTreeMap<String, String>;

/// Resolves references against the configured backend.
#[derive(Debug, Clone)]
pub struct Resolver {
    backend: Backend,
}

impl Resolver {
    /// A resolver for the backend the deployment named.
    pub fn new(backend: Backend) -> Self {
        Self { backend }
    }

    /// The `{envVar: value}` map of one pipeline's references, resolved off the runtime.
    ///
    /// `repository` is the staged tree of this sync, which is where the SOPS backend reads; the
    /// OpenBao backend ignores it. Both backends are blocking — one decrypts a file, the other
    /// speaks HTTP with a blocking client — so the work goes to a blocking thread rather than
    /// stalling the reconciler's runtime, which panics when such a client is dropped inside it.
    pub async fn resolve(
        &self,
        repository: &Path,
        references: &[SecretRef],
    ) -> Result<Environment, SecretError> {
        let resolver = self.clone();
        let repository = repository.to_path_buf();
        let references = references.to_vec();
        match tokio::task::spawn_blocking(move || resolver.environment(&repository, &references))
            .await
        {
            Ok(resolved) => resolved,
            Err(err) => Err(SecretError::Unresolved {
                name: String::new(),
                reason: format!("the resolver did not finish: {err}"),
            }),
        }
    }

    /// [`resolve`](Self::resolve), on the caller's thread. Blocking: not for a runtime.
    pub fn environment(
        &self,
        repository: &Path,
        references: &[SecretRef],
    ) -> Result<Environment, SecretError> {
        if references.is_empty() {
            return Ok(Environment::new());
        }
        for reference in references {
            if reference.env_var.is_none() {
                return Err(SecretError::NoEnvVar {
                    name: reference.name.clone(),
                });
            }
        }
        match &self.backend {
            Backend::Sops { age_key_file } => self.resolve_with_sops(age_key_file, repository, references),
            Backend::OpenBao {
                address,
                role,
                jwt_path,
            } => self.resolve_with_openbao(address, role, jwt_path, references),
        }
    }

    fn resolve_with_sops(
        &self,
        age_key_file: &Path,
        repository: &Path,
        references: &[SecretRef],
    ) -> Result<Environment, SecretError> {
        let identities =
            identities_from_file(age_key_file).map_err(|err| SecretError::Unresolved {
                name: references[0].name.clone(),
                reason: format!("the age identity is unreadable: {err}"),
            })?;
        let store = SecretStore::load_dir(repository, &identities).map_err(|err| {
            SecretError::Unresolved {
                name: references[0].name.clone(),
                reason: err.to_string(),
            }
        })?;
        let mut environment = Environment::new();
        for reference in references {
            let value = store
                .resolve(reference)
                .map_err(|err| SecretError::Unresolved {
                    name: reference.name.clone(),
                    reason: err.to_string(),
                })?;
            environment.insert(
                reference.env_var.clone().unwrap_or_default(),
                value.expose().to_owned(),
            );
        }
        Ok(environment)
    }

    fn resolve_with_openbao(
        &self,
        address: &str,
        role: &str,
        jwt_path: &Path,
        references: &[SecretRef],
    ) -> Result<Environment, SecretError> {
        let unresolved = |name: &str, err: openbao::BaoError| SecretError::Unresolved {
            name: name.to_owned(),
            reason: err.to_string(),
        };
        let settings = openbao::Settings::new(role);
        let jwt = openbao::service_account_jwt(jwt_path)
            .map_err(|err| unresolved(&references[0].name, err))?;
        let mut api = HttpBao::new(address).map_err(|err| unresolved(&references[0].name, err))?;
        let session = openbao::login(&mut api, &settings, &jwt)
            .map_err(|err| unresolved(&references[0].name, err))?;
        let store = BaoStore::fetch(&api, &settings, &session, references)
            .map_err(|err| unresolved(&references[0].name, err))?;
        let resolved = openbao::environment(references, &store)
            .map_err(|err| unresolved(&references[0].name, err))?;
        Ok(resolved
            .into_iter()
            .map(|(variable, value)| (variable, value.expose().to_owned()))
            .collect())
    }
}

/// Every pipeline's environment, merged into the one the runner gets (T-0927).
///
/// The pipelines are taken in the order the caller reads them, which is the mirror's: a clash is
/// reported against whichever came first, and both sides are named so neither author has to
/// guess. A clash refuses the *later* pipeline only, so one bad manifest does not stop the rest.
#[derive(Debug, Default)]
pub struct RunnerEnvironment {
    values: Environment,
    /// Which pipeline claimed each variable, for the clash message.
    claimed_by: BTreeMap<String, String>,
    /// The pipelines that could not be resolved, by `(project, name)`, with the reason.
    refused: BTreeMap<(String, String), String>,
}

impl RunnerEnvironment {
    /// Adds one pipeline's resolved environment, or the reason it has none.
    pub fn add(
        &mut self,
        project: &str,
        pipeline: &str,
        resolved: Result<Environment, SecretError>,
    ) {
        let environment = match resolved {
            Ok(environment) => environment,
            Err(err) => {
                self.refused
                    .insert((project.to_owned(), pipeline.to_owned()), err.to_string());
                return;
            }
        };
        let owner = format!("{project}/{pipeline}");
        for (variable, value) in &environment {
            match (self.values.get(variable), self.claimed_by.get(variable)) {
                // The same value from the same reference is two pipelines reading one
                // credential, which is ordinary and not a clash.
                (Some(held), _) if held == value => continue,
                (Some(_), Some(other)) => {
                    let err = SecretError::Clash {
                        variable: variable.clone(),
                        pipeline: owner.clone(),
                        other: other.clone(),
                    };
                    self.refused
                        .insert((project.to_owned(), pipeline.to_owned()), err.to_string());
                    return;
                }
                _ => {}
            }
        }
        for (variable, value) in environment {
            self.claimed_by.insert(variable.clone(), owner.clone());
            self.values.insert(variable, value);
        }
    }

    /// The pipelines that must not be deployed, with the reason for each.
    pub fn refused(&self) -> &BTreeMap<(String, String), String> {
        &self.refused
    }

    /// Whether any value was resolved at all.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The variables in the Secret, sorted. Names are not secret; values are.
    pub fn variables(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }

    /// The Secret the runner mounts.
    ///
    /// `stringData`, so the API server encodes it and this process never holds a second copy of
    /// the value; the reconciler's field manager, so an edit in the cluster is corrected on the
    /// next sync.
    pub fn secret(&self, namespace: &str) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {
                "name": SECRET_NAME,
                "namespace": namespace,
                "labels": {
                    "app.kubernetes.io/managed-by": "joinedcontext-portal",
                    "joinedcontext.com/pipeline-secrets": "true",
                },
            },
            "type": "Opaque",
            "stringData": self.values,
        })
    }

    /// A fingerprint of the values, for the annotation that rolls the runner.
    ///
    /// An environment variable is read once, when the pod starts, so a rotated credential
    /// reaches a running pipeline only with a restart. The hash is of the values as well as the
    /// names: a rotation that keeps the variable has to roll the runner too. It is a SHA-256 of
    /// a canonical rendering, so it is stable across runs and says nothing about the values.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for (variable, value) in &self.values {
            hasher.update(variable.as_bytes());
            hasher.update([0]);
            hasher.update(value.as_bytes());
            hasher.update([0]);
        }
        format!("{:x}", hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(pairs: &[(&str, &str)]) -> Environment {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn two_pipelines_of_one_runner_share_an_environment() {
        let mut runner = RunnerEnvironment::default();
        runner.add(
            "helsinki",
            "bikes",
            Ok(resolved(&[("MQTT_PASSWORD", "one")])),
        );
        runner.add("helsinki", "air", Ok(resolved(&[("HTTP_TOKEN", "two")])));

        assert!(runner.refused().is_empty());
        assert_eq!(
            runner.variables().collect::<Vec<_>>(),
            vec!["HTTP_TOKEN", "MQTT_PASSWORD"]
        );
        let secret = runner.secret("dev");
        assert_eq!(secret["metadata"]["name"], SECRET_NAME);
        assert_eq!(secret["stringData"]["MQTT_PASSWORD"], "one");
    }

    #[test]
    fn one_variable_claimed_twice_refuses_the_later_pipeline_and_keeps_the_first() {
        let mut runner = RunnerEnvironment::default();
        runner.add("helsinki", "bikes", Ok(resolved(&[("TOKEN", "the-first")])));
        runner.add("helsinki", "air", Ok(resolved(&[("TOKEN", "another")])));

        assert_eq!(runner.secret("dev")["stringData"]["TOKEN"], "the-first");
        let reason = runner
            .refused()
            .get(&("helsinki".to_owned(), "air".to_owned()))
            .expect("the later pipeline is refused");
        assert!(reason.contains("TOKEN"), "{reason}");
        assert!(reason.contains("helsinki/bikes"), "{reason}");
        assert!(!reason.contains("another"), "the refusal repeats the value");
    }

    #[test]
    fn two_pipelines_reading_one_credential_is_not_a_clash() {
        let mut runner = RunnerEnvironment::default();
        runner.add("helsinki", "bikes", Ok(resolved(&[("TOKEN", "shared")])));
        runner.add("helsinki", "air", Ok(resolved(&[("TOKEN", "shared")])));
        assert!(runner.refused().is_empty());
    }

    #[test]
    fn a_reference_that_does_not_resolve_refuses_its_pipeline_alone() {
        let mut runner = RunnerEnvironment::default();
        runner.add("helsinki", "bikes", Ok(resolved(&[("TOKEN", "one")])));
        runner.add(
            "helsinki",
            "air",
            Err(SecretError::Unresolved {
                name: "mqtt-mesto".to_owned(),
                reason: "no such secret".to_owned(),
            }),
        );

        assert_eq!(runner.refused().len(), 1);
        assert_eq!(runner.variables().collect::<Vec<_>>(), vec!["TOKEN"]);
    }

    #[test]
    fn a_reference_without_an_env_var_is_refused_before_a_backend_is_asked() {
        let resolver = Resolver::new(Backend::Sops {
            age_key_file: PathBuf::from("/nonexistent"),
        });
        let error = resolver
            .environment(
                Path::new("/nonexistent"),
                &[SecretRef {
                    name: "mqtt-mesto".to_owned(),
                    key: Some("password".to_owned()),
                    env_var: None,
                }],
            )
            .expect_err("no envVar");
        assert!(matches!(error, SecretError::NoEnvVar { .. }), "{error}");
    }

    #[test]
    fn no_reference_needs_no_backend() {
        let resolver = Resolver::new(Backend::Sops {
            age_key_file: PathBuf::from("/nonexistent"),
        });
        assert_eq!(
            resolver.environment(Path::new("/nonexistent"), &[]),
            Ok(Environment::new())
        );
    }

    #[test]
    fn the_fingerprint_follows_the_value_and_not_only_the_name() {
        let mut one = RunnerEnvironment::default();
        one.add("helsinki", "bikes", Ok(resolved(&[("TOKEN", "before")])));
        let mut two = RunnerEnvironment::default();
        two.add("helsinki", "bikes", Ok(resolved(&[("TOKEN", "after")])));
        assert_ne!(one.fingerprint(), two.fingerprint());
        assert!(!one.fingerprint().contains("before"));
    }
}
