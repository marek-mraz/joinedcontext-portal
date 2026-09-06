pub mod selector;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// The manifest contract is jc-core's (T-0249): metadata, phases, conditions, scopes and the kind
// catalogue come from the tagged crate. What stays here is the kind-generic view the resource API
// needs and jc-core does not have: an envelope whose `spec` is untyped JSON, and the status the
// Portal computes for it.
pub use jc_core::{Condition, KindInfo, ObjectMeta, Phase, Scope, API_VERSION};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResourceEnvelope {
    pub api_version: String,
    pub kind: String,
    #[schema(schema_with = crate::openapi::object_meta_ref)]
    pub metadata: ObjectMeta,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub spec: serde_json::Value,
    /// Never read from Git, always computed by the Portal API (MF-04).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

impl ResourceEnvelope {
    pub fn strip_status(&mut self) {
        self.status = None;
    }

    pub fn key(&self) -> ResourceKey {
        ResourceKey {
            namespace: self.metadata.namespace.clone().unwrap_or_default(),
            kind: self.kind.clone(),
            name: self.metadata.name.clone(),
        }
    }
}

/// Metadata the Portal accepts on a write or from the mirror: jc-core's DNS-1123 and namespace
/// rules, plus a namespace that is present. jc-core checks the namespace on its typed envelope;
/// the kind-generic envelope here has to ask for it itself.
pub fn validate_meta(meta: &ObjectMeta) -> Result<(), String> {
    meta.validate().map_err(|e| e.to_string())?;
    match &meta.namespace {
        Some(ns) if !ns.is_empty() => Ok(()),
        _ => Err("namespace must be set and non-empty".to_string()),
    }
}

pub fn is_dns1123(name: &str) -> bool {
    jc_core::names::validate_dns1123_label(name).is_ok()
}

/// The phase as it appears on the wire, for `fieldSelector=status.phase=Live`. Exhaustive on
/// purpose: a new jc-core phase must be given its name here before it compiles.
pub fn phase_str(phase: Phase) -> &'static str {
    match phase {
        Phase::Draft => "Draft",
        Phase::Pending => "Pending",
        Phase::Deploying => "Deploying",
        Phase::Live => "Live",
        Phase::Error => "Error",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
/// The status the Portal API reports (MF-04). It is `jc_core::Status` plus `sourceUrl` and a phase
/// that is always known; it stays a Portal type until jc-core carries `sourceUrl` too (docs API/01
/// section 6 is the contract), then it becomes a re-export like its neighbours.
pub struct Status {
    #[schema(schema_with = crate::openapi::phase_ref)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_revision: Option<String>,
    /// Forge page of the file this manifest was read from, so a view can link "Source"
    /// without knowing where a kind lives in the repository. Computed, never read from Git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(schema_with = crate::openapi::conditions_ref)]
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResourceKey {
    pub namespace: String,
    pub kind: String,
    pub name: String,
}

/// Kinds the specification defines and the Portal already serves, but `jc-core-v0.4.0` does not
/// implement yet: Dashboard and Layer (UI-17, UI-18, Architecture/10), Entity seeds, Subscription
/// and ContextSourceRegistration (Architecture/06 section 3, DS-16). Paths follow Architecture/06.
/// Blueprint left this list when jc-core-v0.4.0 took the kind over, which is exactly the move the
/// next paragraph describes.
///
/// They live apart from [`jc_core::KINDS`] so the difference stays visible: when @platform adds a
/// kind to jc-core, its row moves out of this list and nothing else changes.
pub const PORTAL_ONLY_KINDS: &[KindInfo] = &[
    // jc-core v0.4.0 predates the kind (platform 6343c87, MF-35); its row moves out of this
    // list the moment the Portal depends on a tag that carries `DataSourceSpec`, and the
    // resource API keeps serving `datasources` at the same path either way.
    KindInfo {
        kind: "DataSource",
        plural: "datasources",
        scope: Scope::Project,
        path_template: "projects/{project}/datasources/{name}.yaml",
    },
    KindInfo {
        kind: "Subscription",
        plural: "subscriptions",
        scope: Scope::Project,
        path_template: "projects/{project}/spaces/{space}/subscriptions/{name}.yaml",
    },
    KindInfo {
        kind: "ContextSourceRegistration",
        plural: "contextsourceregistrations",
        scope: Scope::Project,
        path_template: "projects/{project}/spaces/{space}/registrations/{name}.yaml",
    },
    KindInfo {
        kind: "Entity",
        plural: "entities",
        scope: Scope::Project,
        path_template: "projects/{project}/spaces/{space}/entities/seed/{name}.yaml",
    },
    KindInfo {
        kind: "Dashboard",
        plural: "dashboards",
        scope: Scope::Project,
        path_template: "projects/{project}/dashboards/{name}.yaml",
    },
    KindInfo {
        kind: "Layer",
        plural: "layers",
        scope: Scope::Project,
        path_template: "projects/{project}/dashboards/{name}.yaml",
    },
];

/// Every kind the resource API serves: the jc-core catalogue first, the Portal-only kinds after,
/// so a plural that exists in both always resolves to the crate's row.
pub fn kinds() -> impl Iterator<Item = &'static KindInfo> {
    jc_core::KINDS.iter().chain(PORTAL_ONLY_KINDS.iter())
}

pub fn by_plural(plural: &str) -> Option<&'static KindInfo> {
    kinds().find(|info| info.plural == plural)
}

pub fn by_kind(kind: &str) -> Option<&'static KindInfo> {
    kinds().find(|info| info.kind == kind)
}

/// [`KindInfo::repo_path`] with one refusal on top: a placeholder must never be left behind, since
/// a manifest written to `.../spaces/{space}/...` would be unreachable for the reconciler and
/// invisible in Gitea.
pub fn repository_path(
    info: &KindInfo,
    project: &str,
    space: Option<&str>,
    name: &str,
) -> Result<String, String> {
    if info.path_template.contains("{space}") && space.is_none_or(str::is_empty) {
        return Err(format!(
            "space is required for kind '{}' ({})",
            info.kind, info.path_template
        ));
    }
    Ok(info.repo_path(project, space.unwrap_or_default(), name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_round_trips_plural_kind_plural() {
        for entry in kinds() {
            let by_p = by_plural(entry.plural).expect("find by plural");
            // Policy and ScopeDefinition deliberately share the plural "policies"; every other
            // kind must come back as itself.
            assert!(by_p.plural == entry.plural);
            let by_k = by_kind(entry.kind).expect("find by kind");
            assert_eq!(by_k.kind, entry.kind);
            assert_eq!(by_k.plural, entry.plural);
        }
    }

    #[test]
    fn every_kind_the_portal_serves_is_jc_core_or_declared_portal_only() {
        // No third list may creep in: a plural resolves through jc-core's catalogue or through
        // the explicit PORTAL_ONLY_KINDS rows, nothing else (T-0249).
        for info in kinds() {
            assert!(
                jc_core::KINDS.contains(info) || PORTAL_ONLY_KINDS.contains(info),
                "{} comes from neither list",
                info.kind
            );
        }
        let from_jc_core: Vec<&str> = jc_core::KINDS.iter().map(|k| k.kind).collect();
        assert_eq!(
            from_jc_core,
            vec![
                "Organization",
                "Project",
                "ContextSpace",
                "DataModel",
                "Mapping",
                "Policy",
                "ScopeDefinition",
                "Endpoint",
                "SharedSpaceReference",
                "ServiceAccount",
                "Pipeline",
                "App",
                "Blueprint",
                "DataSpaceParticipant",
                "DataOffer",
                "DataAgreement",
                "SyncSource",
                "Bundle",
            ],
            "jc-core's catalogue changed: check PORTAL_ONLY_KINDS and the UI navigation"
        );
    }

    #[test]
    fn portal_only_kinds_never_shadow_a_jc_core_kind() {
        for extra in PORTAL_ONLY_KINDS {
            assert!(
                !jc_core::KINDS.iter().any(|k| k.kind == extra.kind),
                "{} is in jc-core now: move its row out of PORTAL_ONLY_KINDS",
                extra.kind
            );
            if let Some(clash) = jc_core::KINDS.iter().find(|k| k.plural == extra.plural) {
                panic!(
                    "portal-only {} claims the plural '{}' that jc-core gives {}",
                    extra.kind, extra.plural, clash.kind
                );
            }
        }
    }

    #[test]
    fn a_rendered_path_never_keeps_a_placeholder() {
        for info in kinds() {
            let path = repository_path(info, "ovzdusie", Some("ovzdusie"), "demo")
                .unwrap_or_else(|e| panic!("{}: {e}", info.kind));
            assert!(
                !path.contains('{') && !path.contains('}'),
                "{} rendered to {path}",
                info.kind
            );
        }
    }

    #[test]
    fn spaces_maps_to_context_space() {
        let info = by_plural("spaces").expect("find spaces");
        assert_eq!(info.kind, "ContextSpace");
    }

    #[test]
    fn demo_endpoint_path_is_expected() {
        let info = by_kind("Endpoint").expect("find Endpoint");
        let path = repository_path(info, "ovzdusie", Some("ovzdusie"), "public-air")
            .expect("endpoint path");
        assert_eq!(
            path,
            "projects/ovzdusie/spaces/ovzdusie/endpoints/public-air.yaml"
        );
    }

    #[test]
    fn space_scoped_kind_without_space_errors() {
        let info = by_kind("Endpoint").expect("find Endpoint");
        assert!(repository_path(info, "ovzdusie", None, "public-air").is_err());
    }

    #[test]
    fn dns1123_validation_rules() {
        assert!(is_dns1123("public-air"));
        assert!(is_dns1123("a"));
        assert!(is_dns1123("0"));
        assert!(is_dns1123("air-quality-01"));

        assert!(!is_dns1123("Public_Air"));
        assert!(!is_dns1123("-x"));
        assert!(!is_dns1123("x-"));
        assert!(!is_dns1123(""));
        let name_64 = "a".repeat(64);
        assert!(!is_dns1123(&name_64));
        let name_63 = "a".repeat(63);
        assert!(is_dns1123(&name_63));
    }

    #[test]
    fn object_meta_validate() {
        let valid = ObjectMeta {
            name: "public-air".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        };
        assert!(validate_meta(&valid).is_ok());

        let missing_ns = ObjectMeta {
            name: "public-air".into(),
            namespace: None,
            ..Default::default()
        };
        assert!(validate_meta(&missing_ns).is_err());

        let empty_ns = ObjectMeta {
            name: "public-air".into(),
            namespace: Some("".into()),
            ..Default::default()
        };
        assert!(validate_meta(&empty_ns).is_err());

        let bad_name = ObjectMeta {
            name: "Public_Air".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        };
        assert!(validate_meta(&bad_name).is_err());
    }
}
