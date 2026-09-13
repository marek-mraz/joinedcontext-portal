//! The `AgentProfile` one run executes, read out of the mirror (AG-47…AG-50).
//!
//! Read from the untyped spec rather than through jc-core's typed kind, the way the rest of the
//! Portal reads a manifest: the mirror holds manifests as they are in Git, and the pinned
//! `jc-core` of this build may be older than the kind. Every field the runner and the proxy act
//! on is required here, so a profile that is missing one is a 503 rather than a run with a
//! guessed budget.

use serde_json::Value;

use crate::api::blueprints::ORG_NAMESPACE;
use crate::error::ApiError;
use crate::store::Mirror;

/// What the platform needs to know to run one agent: what to start, what to call, and what it
/// may spend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub role: String,
    /// Image reference pinned by digest, ready for a pod spec (AG-49).
    pub image: String,
    pub model_name: String,
    /// `anthropic` or `openai-compatible`: which body the model call carries (AG-53).
    pub model_provider: String,
    pub max_tokens_per_run: u64,
    pub steps_per_run: u32,
    pub requests_per_minute: u32,
    pub max_response_bytes: u64,
    /// Bare hostnames the package route may reach (AG-50).
    pub allowed_hosts: Vec<String>,
    pub cpu: String,
    pub memory: String,
    pub ephemeral_storage: String,
}

impl Profile {
    /// The profile of this name, or the 503 a Portal answers when it has none (AG-47).
    pub fn load(mirror: &Mirror, name: &str) -> Result<Self, ApiError> {
        let envelope = mirror
            .get(ORG_NAMESPACE, "AgentProfile", name)
            .ok_or_else(|| {
                ApiError::Unavailable(format!(
                    "agent profile '{name}' is not in this organization's configuration"
                ))
            })?;
        let spec = &envelope.spec;

        let role = string_at(spec, &["role"], name)?;
        let image = string_at(spec, &["runtime", "image"], name)?;
        let digest = string_at(spec, &["runtime", "digest"], name)?;
        // AG-49: a tag is not an identity. `name:tag@sha256:…` is a legal reference and the
        // digest is what resolves it, so the tag is left in place for a human reading a pod
        // spec; a digest already in the manifest's image is replaced by the declared one,
        // because `spec.runtime.digest` is the field admission checks.
        let image = match image.split_once('@') {
            Some((reference, _)) => format!("{reference}@{digest}"),
            None => format!("{image}@{digest}"),
        };

        Ok(Self {
            name: name.to_owned(),
            role,
            image,
            model_name: string_at(spec, &["model", "name"], name)?,
            model_provider: string_at(spec, &["model", "provider"], name)?,
            max_tokens_per_run: u64_at(spec, &["model", "maxTokensPerRun"], name)?,
            steps_per_run: u64_at(spec, &["limits", "stepsPerRun"], name)? as u32,
            requests_per_minute: u64_at(spec, &["limits", "requestsPerMinute"], name)? as u32,
            max_response_bytes: u64_at(spec, &["limits", "maxResponseBytes"], name)?,
            allowed_hosts: spec
                .pointer("/egress/allowedHosts")
                .and_then(Value::as_array)
                .map(|hosts| {
                    hosts
                        .iter()
                        .filter_map(|host| host.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            cpu: string_at(spec, &["workspace", "cpu"], name)?,
            memory: string_at(spec, &["workspace", "memory"], name)?,
            ephemeral_storage: string_at(spec, &["workspace", "ephemeralStorage"], name)?,
        })
    }

    /// Whether this profile may build an application. A `steward` profile has no workspace and
    /// no egress, so starting a builder run on one is the caller's mistake (AG-26, AG-47).
    pub fn is_builder(&self) -> bool {
        self.role == "builder"
    }
}

fn string_at(spec: &Value, path: &[&str], profile: &str) -> Result<String, ApiError> {
    spec.pointer(&pointer(path))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| missing(path, profile))
}

fn u64_at(spec: &Value, path: &[&str], profile: &str) -> Result<u64, ApiError> {
    spec.pointer(&pointer(path))
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| missing(path, profile))
}

fn pointer(path: &[&str]) -> String {
    path.iter().map(|part| format!("/{part}")).collect()
}

fn missing(path: &[&str], profile: &str) -> ApiError {
    ApiError::Unavailable(format!(
        "agent profile '{profile}' has no usable 'spec{}'",
        pointer(path).replace('/', ".")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::ResourceEnvelope;

    fn profile_spec() -> Value {
        serde_json::json!({
            "role": "builder",
            "runtime": {
                "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            },
            "model": { "provider": "anthropic", "name": "claude-sonnet-5", "maxTokensPerRun": 400000 },
            "limits": {
                "stepsPerRun": 120,
                "wallClock": "PT20M",
                "concurrentRunsPerOrganization": 2,
                "requestsPerMinute": 60,
                "maxResponseBytes": 2097152
            },
            "egress": { "allowedHosts": ["registry.npmjs.org", "static.crates.io"] },
            "tools": ["shell", "npm", "git"],
            "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
        })
    }

    fn mirror_with(spec: Value) -> Mirror {
        let mirror = Mirror::new();
        let envelope: ResourceEnvelope = serde_json::from_value(serde_json::json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "AgentProfile",
            "metadata": { "name": "app-builder", "namespace": ORG_NAMESPACE },
            "spec": spec,
        }))
        .expect("envelope");
        mirror.upsert(envelope);
        mirror
    }

    #[test]
    fn a_complete_profile_is_read_with_its_image_pinned_by_digest() {
        let profile = Profile::load(&mirror_with(profile_spec()), "app-builder").expect("profile");
        assert!(profile.is_builder());
        assert_eq!(
            profile.image,
            "ghcr.io/all-hands-ai/agent-server:v1.4.0@sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "the declared digest pins the reference, whatever the tag says (AG-49)"
        );
        assert_eq!(profile.model_name, "claude-sonnet-5");
        assert_eq!(profile.max_tokens_per_run, 400_000);
        assert_eq!(profile.requests_per_minute, 60);
        assert_eq!(profile.allowed_hosts.len(), 2);
        assert_eq!(profile.ephemeral_storage, "4Gi");
    }

    #[test]
    fn a_digest_in_the_image_is_replaced_by_the_declared_one() {
        let mut spec = profile_spec();
        spec["runtime"]["image"] = serde_json::json!(
            "ghcr.io/all-hands-ai/agent-server@sha256:0000000000000000000000000000000000000000000000000000000000000000"
        );
        let profile = Profile::load(&mirror_with(spec), "app-builder").expect("profile");
        assert!(
            profile.image.ends_with(
                "@sha256:1111111111111111111111111111111111111111111111111111111111111111"
            ),
            "spec.runtime.digest is the field admission checks, so it wins: {}",
            profile.image
        );
        assert_eq!(profile.image.matches('@').count(), 1);
    }

    #[test]
    fn a_profile_nobody_configured_is_unavailable_rather_than_a_default() {
        let err = Profile::load(&Mirror::new(), "app-builder").expect_err("no profile");
        assert!(matches!(err, ApiError::Unavailable(_)));
    }

    #[test]
    fn a_profile_missing_a_budget_is_unavailable_rather_than_unlimited() {
        let mut spec = profile_spec();
        spec["model"]["maxTokensPerRun"] = serde_json::json!(0);
        let err = Profile::load(&mirror_with(spec), "app-builder").expect_err("no budget");
        match err {
            ApiError::Unavailable(message) => {
                assert!(message.contains("spec.model.maxTokensPerRun"), "{message}")
            }
            other => panic!("expected 503, got {other:?}"),
        }
    }

    #[test]
    fn a_steward_profile_is_not_a_builder() {
        let mut spec = profile_spec();
        spec["role"] = serde_json::json!("steward");
        let profile = Profile::load(&mirror_with(spec), "app-builder").expect("profile");
        assert!(!profile.is_builder());
    }
}
