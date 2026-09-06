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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Organization,
    Project,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathTemplate {
    SpaceDir {
        dir: &'static str,
    },
    ProjectFile {
        dir: &'static str,
    },
    ProjectDirFile {
        dir: &'static str,
        file: &'static str,
    },
    SpaceSelf,
    ProjectSelf,
    OrgFile {
        path: &'static str,
    },
    OrgDirFile {
        dir: &'static str,
        file: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindInfo {
    pub kind: &'static str,
    pub plural: &'static str,
    pub scope: Scope,
    pub path: PathTemplate,
}

pub const KIND_REGISTRY: &[KindInfo] = &[
    KindInfo {
        kind: "Organization",
        plural: "organizations",
        scope: Scope::Organization,
        path: PathTemplate::OrgFile { path: "org.yaml" },
    },
    KindInfo {
        kind: "Project",
        plural: "projects",
        scope: Scope::Project,
        path: PathTemplate::ProjectSelf,
    },
    KindInfo {
        kind: "ContextSpace",
        plural: "spaces",
        scope: Scope::Project,
        path: PathTemplate::SpaceSelf,
    },
    KindInfo {
        kind: "DataModel",
        plural: "datamodels",
        scope: Scope::Space,
        path: PathTemplate::SpaceDir { dir: "datamodels" },
    },
    KindInfo {
        kind: "Policy",
        plural: "policies",
        scope: Scope::Space,
        path: PathTemplate::SpaceDir { dir: "policies" },
    },
    KindInfo {
        kind: "ScopeDefinition",
        plural: "scopedefinitions",
        scope: Scope::Space,
        path: PathTemplate::SpaceDir { dir: "policies" },
    },
    KindInfo {
        kind: "Subscription",
        plural: "subscriptions",
        scope: Scope::Space,
        path: PathTemplate::SpaceDir {
            dir: "subscriptions",
        },
    },
    KindInfo {
        kind: "ContextSourceRegistration",
        plural: "contextsourceregistrations",
        scope: Scope::Space,
        path: PathTemplate::SpaceDir {
            dir: "registrations",
        },
    },
    KindInfo {
        kind: "Entity",
        plural: "entities",
        scope: Scope::Space,
        path: PathTemplate::SpaceDir {
            dir: "entities/seed",
        },
    },
    KindInfo {
        kind: "Endpoint",
        plural: "endpoints",
        scope: Scope::Space,
        path: PathTemplate::SpaceDir { dir: "endpoints" },
    },
    KindInfo {
        kind: "Mapping",
        plural: "mappings",
        scope: Scope::Space,
        path: PathTemplate::SpaceDir {
            dir: "datamodels/mappings",
        },
    },
    KindInfo {
        kind: "Pipeline",
        plural: "pipelines",
        scope: Scope::Project,
        path: PathTemplate::ProjectDirFile {
            dir: "pipelines",
            file: "pipeline.yaml",
        },
    },
    KindInfo {
        kind: "Dashboard",
        plural: "dashboards",
        scope: Scope::Project,
        path: PathTemplate::ProjectFile { dir: "dashboards" },
    },
    KindInfo {
        kind: "Layer",
        plural: "layers",
        scope: Scope::Project,
        path: PathTemplate::ProjectFile { dir: "dashboards" },
    },
    KindInfo {
        kind: "SharedSpaceReference",
        plural: "sharedspacereferences",
        scope: Scope::Project,
        path: PathTemplate::ProjectFile { dir: "shared" },
    },
    KindInfo {
        kind: "ServiceAccount",
        plural: "serviceaccounts",
        scope: Scope::Project,
        path: PathTemplate::ProjectFile {
            dir: "access/serviceaccounts",
        },
    },
    KindInfo {
        kind: "SyncSource",
        plural: "syncsources",
        scope: Scope::Project,
        path: PathTemplate::ProjectFile { dir: "sync" },
    },
    KindInfo {
        kind: "App",
        plural: "apps",
        scope: Scope::Project,
        path: PathTemplate::ProjectDirFile {
            dir: "apps",
            file: "app.yaml",
        },
    },
    KindInfo {
        kind: "Blueprint",
        plural: "blueprints",
        scope: Scope::Organization,
        path: PathTemplate::OrgDirFile {
            dir: "blueprints",
            file: "blueprint.yaml",
        },
    },
    KindInfo {
        kind: "DataSpaceParticipant",
        plural: "dataspaceparticipants",
        scope: Scope::Organization,
        path: PathTemplate::OrgFile {
            path: "dataspace/participant.yaml",
        },
    },
    KindInfo {
        kind: "DataOffer",
        plural: "dataoffers",
        scope: Scope::Space,
        path: PathTemplate::SpaceDir {
            dir: "dataspace/offers",
        },
    },
    KindInfo {
        kind: "DataAgreement",
        plural: "dataagreements",
        scope: Scope::Project,
        path: PathTemplate::ProjectFile {
            dir: "dataspace/agreements",
        },
    },
];

pub fn by_plural(plural: &str) -> Option<&'static KindInfo> {
    KIND_REGISTRY.iter().find(|info| info.plural == plural)
}

pub fn by_kind(kind: &str) -> Option<&'static KindInfo> {
    KIND_REGISTRY.iter().find(|info| info.kind == kind)
}

pub fn repository_path(
    info: &KindInfo,
    project: &str,
    space: Option<&str>,
    name: &str,
) -> Result<String, String> {
    match info.path {
        PathTemplate::SpaceDir { dir } => {
            let space = space.ok_or_else(|| {
                format!("space is required for space-scoped kind '{}'", info.kind)
            })?;
            Ok(format!(
                "projects/{project}/spaces/{space}/{dir}/{name}.yaml"
            ))
        }
        PathTemplate::ProjectFile { dir } => Ok(format!("projects/{project}/{dir}/{name}.yaml")),
        PathTemplate::ProjectDirFile { dir, file } => {
            Ok(format!("projects/{project}/{dir}/{name}/{file}"))
        }
        PathTemplate::SpaceSelf => Ok(format!("projects/{project}/spaces/{name}/space.yaml")),
        PathTemplate::ProjectSelf => Ok(format!("projects/{project}/project.yaml")),
        PathTemplate::OrgFile { path } => Ok(path.to_string()),
        PathTemplate::OrgDirFile { dir, file } => Ok(format!("{dir}/{name}/{file}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_round_trips_plural_kind_plural() {
        for entry in KIND_REGISTRY {
            let by_p = by_plural(entry.plural).expect("find by plural");
            assert_eq!(by_p.kind, entry.kind);
            let by_k = by_kind(entry.kind).expect("find by kind");
            assert_eq!(by_k.plural, entry.plural);
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
