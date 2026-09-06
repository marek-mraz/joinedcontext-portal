pub mod selector;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

pub const API_VERSION: &str = "joinedcontext.com/v1alpha1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResourceEnvelope {
    pub api_version: String,
    pub kind: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ObjectMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<BTreeMap<String, String>>,
}

impl ObjectMeta {
    pub fn validate(&self) -> Result<(), String> {
        if !is_dns1123(&self.name) {
            return Err(format!(
                "name '{}' is not DNS-1123 compliant (lowercase alphanumeric and '-', 1..=63 chars, start/end alphanumeric)",
                self.name
            ));
        }
        match &self.namespace {
            Some(ns) if !ns.is_empty() => Ok(()),
            _ => Err("namespace must be set and non-empty".to_string()),
        }
    }
}

pub fn is_dns1123(name: &str) -> bool {
    if name.is_empty() || name.len() > 63 {
        return false;
    }
    let bytes = name.as_bytes();
    let is_alphanumeric = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !is_alphanumeric(bytes[0]) || !is_alphanumeric(bytes[bytes.len() - 1]) {
        return false;
    }
    bytes.iter().all(|&b| is_alphanumeric(b) || b == b'-')
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Status {
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub enum Phase {
    Draft,
    Pending,
    Deploying,
    Live,
    Error,
}

impl Phase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::Pending => "Pending",
            Self::Deploying => "Deploying",
            Self::Live => "Live",
            Self::Error => "Error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Condition {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: String,
    pub last_transition_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResourceKey {
    pub namespace: String,
    pub kind: String,
    pub name: String,
}

/// Mirrors `jc_core::envelope::Scope`. Space-ness is not a scope: it is the `{space}`
/// placeholder in the path template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Organization,
    Project,
}

/// Mirrors `jc_core::registry::KindInfo` field for field, so T-0249 can delete this module
/// and re-export the real one without touching a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindInfo {
    pub kind: &'static str,
    pub plural: &'static str,
    pub scope: Scope,
    /// Repository path with the `{project}`, `{space}` and `{name}` placeholders.
    pub path_template: &'static str,
}

/// The 17 kinds of `jc-core-v0.1.0`, copied constant for constant from
/// `jc_core::registry::KINDS` (a6e9d1bd). Do not edit a row here to fix behaviour: fix it in
/// jc-core, retag, and let T-0249 replace the whole list.
pub const JC_CORE_KINDS: &[KindInfo] = &[
    KindInfo {
        kind: "Organization",
        plural: "organizations",
        scope: Scope::Organization,
        path_template: "org.yaml",
    },
    KindInfo {
        kind: "Project",
        plural: "projects",
        scope: Scope::Organization,
        path_template: "projects/{name}/project.yaml",
    },
    KindInfo {
        kind: "ContextSpace",
        plural: "spaces",
        scope: Scope::Project,
        path_template: "projects/{project}/spaces/{name}/space.yaml",
    },
    KindInfo {
        kind: "DataModel",
        plural: "datamodels",
        scope: Scope::Project,
        path_template: "projects/{project}/spaces/{space}/datamodels/{name}.yaml",
    },
    KindInfo {
        kind: "Mapping",
        plural: "mappings",
        scope: Scope::Project,
        path_template: "projects/{project}/spaces/{space}/datamodels/mappings/{name}.yaml",
    },
    KindInfo {
        kind: "Policy",
        plural: "policies",
        scope: Scope::Project,
        path_template: "projects/{project}/spaces/{space}/policies/{name}.yaml",
    },
    KindInfo {
        kind: "ScopeDefinition",
        // jc-core gives ScopeDefinition the same plural as Policy, so `by_plural("policies")`
        // answers Policy. Kept as-is on purpose: the mirror must not diverge from the crate.
        plural: "policies",
        scope: Scope::Project,
        path_template: "projects/{project}/policies/{name}.yaml",
    },
    KindInfo {
        kind: "Endpoint",
        plural: "endpoints",
        scope: Scope::Project,
        path_template: "projects/{project}/spaces/{space}/endpoints/{name}.yaml",
    },
    KindInfo {
        kind: "SharedSpaceReference",
        plural: "shared",
        scope: Scope::Project,
        path_template: "projects/{project}/shared/{name}.yaml",
    },
    KindInfo {
        kind: "ServiceAccount",
        plural: "serviceaccounts",
        scope: Scope::Project,
        path_template: "projects/{project}/access/serviceaccounts/{name}.yaml",
    },
    KindInfo {
        kind: "Pipeline",
        plural: "pipelines",
        scope: Scope::Project,
        path_template: "projects/{project}/pipelines/{name}/pipeline.yaml",
    },
    KindInfo {
        kind: "App",
        plural: "apps",
        scope: Scope::Project,
        path_template: "projects/{project}/apps/{name}/app.yaml",
    },
    KindInfo {
        kind: "DataSpaceParticipant",
        plural: "dataspaceparticipants",
        scope: Scope::Organization,
        path_template: "dataspace/participant.yaml",
    },
    KindInfo {
        kind: "DataOffer",
        plural: "dataoffers",
        scope: Scope::Project,
        path_template: "projects/{project}/spaces/{space}/dataspace/offers/{name}.yaml",
    },
    KindInfo {
        kind: "DataAgreement",
        plural: "dataagreements",
        scope: Scope::Project,
        path_template: "projects/{project}/dataspace/agreements/{name}.yaml",
    },
    KindInfo {
        kind: "SyncSource",
        plural: "syncsources",
        scope: Scope::Project,
        path_template: "projects/{project}/sync/{name}.yaml",
    },
    KindInfo {
        kind: "Bundle",
        plural: "bundles",
        scope: Scope::Organization,
        path_template: "bundle.yaml",
    },
];

/// Kinds the specification defines and the Portal already serves, but `jc-core-v0.1.0` does not
/// implement yet: Dashboard and Layer (UI-17, UI-18, Architecture/10), Subscription and
/// ContextSourceRegistration (Architecture/06 section 3, DS-16) and Blueprint (API/01 section 4
/// lists it among the organization-level kinds). Paths follow Architecture/06.
///
/// They live apart from [`JC_CORE_KINDS`] so the difference stays visible: when @platform adds a
/// kind to jc-core, its row moves out of this list and nothing else changes.
pub const PORTAL_ONLY_KINDS: &[KindInfo] = &[
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
    KindInfo {
        kind: "Blueprint",
        plural: "blueprints",
        scope: Scope::Organization,
        path_template: "blueprints/{name}/blueprint.yaml",
    },
];

/// Every kind the resource API serves: the jc-core catalogue first, the Portal-only kinds after,
/// so a plural that exists in both always resolves to the crate's row.
pub fn kinds() -> impl Iterator<Item = &'static KindInfo> {
    JC_CORE_KINDS.iter().chain(PORTAL_ONLY_KINDS.iter())
}

pub fn by_plural(plural: &str) -> Option<&'static KindInfo> {
    kinds().find(|info| info.plural == plural)
}

pub fn by_kind(kind: &str) -> Option<&'static KindInfo> {
    kinds().find(|info| info.kind == kind)
}

/// Renders the path template, the way `jc_core::registry::KindInfo::repo_path` does, but refusing
/// to leave a placeholder behind: a manifest written to `.../spaces/{space}/...` would be
/// unreachable for the reconciler and invisible in Gitea.
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
    Ok(info
        .path_template
        .replace("{project}", project)
        .replace("{space}", space.unwrap_or_default())
        .replace("{name}", name))
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
    fn jc_core_catalogue_is_mirrored_completely() {
        let mirrored: Vec<&str> = JC_CORE_KINDS.iter().map(|k| k.kind).collect();
        assert_eq!(
            mirrored,
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
                "DataSpaceParticipant",
                "DataOffer",
                "DataAgreement",
                "SyncSource",
                "Bundle",
            ],
            "the mirror drifted from jc_core::registry::KINDS of jc-core-v0.1.0"
        );
    }

    #[test]
    fn portal_only_kinds_never_shadow_a_jc_core_kind() {
        for extra in PORTAL_ONLY_KINDS {
            assert!(
                !JC_CORE_KINDS.iter().any(|k| k.kind == extra.kind),
                "{} is in jc-core now: move its row out of PORTAL_ONLY_KINDS",
                extra.kind
            );
            if let Some(clash) = JC_CORE_KINDS.iter().find(|k| k.plural == extra.plural) {
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
        assert!(valid.validate().is_ok());

        let missing_ns = ObjectMeta {
            name: "public-air".into(),
            namespace: None,
            ..Default::default()
        };
        assert!(missing_ns.validate().is_err());

        let empty_ns = ObjectMeta {
            name: "public-air".into(),
            namespace: Some("".into()),
            ..Default::default()
        };
        assert!(empty_ns.validate().is_err());

        let bad_name = ObjectMeta {
            name: "Public_Air".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        };
        assert!(bad_name.validate().is_err());
    }
}
