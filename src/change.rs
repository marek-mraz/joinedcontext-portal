//! The `Change` resource returned by every mutation in the portal (MF-12, CC-63).
//!
//! Mutations in joinedcontext are never direct live writes: they are proposed changes
//! routed through Git merge requests and classified into approval lanes (Green, Yellow, Red).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::resource::API_VERSION;

/// The `Change` resource describing a proposed configuration update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub api_version: String,
    pub kind: String,
    pub metadata: ChangeMeta,
    pub status: ChangeStatus,
}

impl Change {
    pub const KIND: &'static str = "Change";

    pub fn new(metadata: ChangeMeta, status: ChangeStatus) -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            kind: Self::KIND.to_string(),
            metadata,
            status,
        }
    }
}

/// Metadata identifying the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeMeta {
    /// Identifier formatted as `chg-` plus eight lowercase hex characters derived
    /// from the merge request.
    pub name: String,
    pub namespace: String,
}

impl ChangeMeta {
    pub fn new(name: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: namespace.into(),
        }
    }

    /// Derives the change metadata from a pull request / merge request number.
    pub fn from_merge_request(mr_number: u64, namespace: impl Into<String>) -> Self {
        Self {
            name: format!("chg-{:08x}", mr_number),
            namespace: namespace.into(),
        }
    }

    /// Derives the change metadata deterministically from a merge request string identifier or URL.
    pub fn from_mr_str(mr: &str, namespace: impl Into<String>) -> Self {
        let parsed_num: Option<u64> = mr
            .rsplit('/')
            .next()
            .and_then(|seg| seg.parse().ok())
            .or_else(|| mr.parse().ok());

        let name = match parsed_num {
            Some(n) => format!("chg-{:08x}", n),
            None => {
                // 32-bit FNV-1a hash producing 8 lowercase hex digits
                let mut hash: u32 = 0x811c_9dc5;
                for b in mr.as_bytes() {
                    hash ^= *b as u32;
                    hash = hash.wrapping_mul(0x0100_0193);
                }
                format!("chg-{:08x}", hash)
            }
        };

        Self {
            name,
            namespace: namespace.into(),
        }
    }
}

/// Approval and reconciliation status of a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangeStatus {
    pub lane: Lane,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_request: Option<String>,
    pub plan: PlanSummary,
    pub phase: ChangePhase,
}

impl ChangeStatus {
    pub fn new(lane: Lane, phase: ChangePhase, plan: PlanSummary) -> Self {
        Self {
            lane,
            merge_request: None,
            plan,
            phase,
        }
    }

    pub fn with_merge_request(mut self, mr: impl Into<String>) -> Self {
        self.merge_request = Some(mr.into());
        self
    }
}

/// Change risk classification lane (CC-63).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Lane {
    Green,
    Yellow,
    Red,
}

/// Lifecycle phase of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub enum ChangePhase {
    PendingApproval,
    Merged,
    Applied,
    Rejected,
}

/// Counts of planned resource mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    #[serde(default)]
    pub create: usize,
    #[serde(default)]
    pub update: usize,
    #[serde(default)]
    pub delete: usize,
}

impl PlanSummary {
    pub fn new(create: usize, update: usize, delete: usize) -> Self {
        Self {
            create,
            update,
            delete,
        }
    }
}

/// Resource mutation operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub enum Operation {
    Create,
    Update,
    Delete,
}

/// Classifies a resource operation into an approval lane per
/// `docs/Architecture/06-configuration-as-code.md` §4 and CC-63.
///
/// Precedence:
/// - `Operation::Delete` is ALWAYS `Red`, without exception (CC-19, CC-39).
/// - `Endpoint` with `spec.audience == "public"` is `Red` (public exposure).
/// - Federation edges and data-space edges (`ContextSourceRegistration`, `SharedSpaceReference`,
///   `DataSpaceParticipant`, `DataOffer`, `DataAgreement`) are `Red`.
/// - Identity and access kinds (`ServiceAccount`, `Policy`, `ScopeDefinition`, `Organization`,
///   `Project`) are `Red`.
/// - `ContextSpace` with `spec.isSandbox == true` is `Green` (ephemeral sandbox, CC-67).
/// - `Dashboard` and `Layer` are `Green`.
/// - Everything else defaults to `Yellow`.
pub fn classify(kind: &str, op: Operation, spec: &serde_json::Value) -> Lane {
    if op == Operation::Delete {
        return Lane::Red;
    }

    if kind == "Endpoint" && spec.get("audience").and_then(|v| v.as_str()) == Some("public") {
        return Lane::Red;
    }

    match kind {
        "ContextSourceRegistration"
        | "SharedSpaceReference"
        | "DataSpaceParticipant"
        | "DataOffer"
        | "DataAgreement"
        | "ServiceAccount"
        | "Policy"
        | "ScopeDefinition"
        | "Organization"
        | "Project" => Lane::Red,

        "ContextSpace" if spec.get("isSandbox").and_then(|v| v.as_bool()) == Some(true) => {
            Lane::Green
        }

        "Dashboard" | "Layer" => Lane::Green,

        _ => Lane::Yellow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deletions_always_land_in_red_lane() {
        let empty_spec = json!({});
        // Deletion always wins over Green kinds
        assert_eq!(
            classify("Dashboard", Operation::Delete, &empty_spec),
            Lane::Red
        );
        assert_eq!(classify("Layer", Operation::Delete, &empty_spec), Lane::Red);
        let sandbox_spec = json!({ "isSandbox": true });
        assert_eq!(
            classify("ContextSpace", Operation::Delete, &sandbox_spec),
            Lane::Red
        );

        // Deletion wins over Yellow kinds
        assert_eq!(
            classify("DataModel", Operation::Delete, &empty_spec),
            Lane::Red
        );
        assert_eq!(
            classify("Pipeline", Operation::Delete, &empty_spec),
            Lane::Red
        );

        // Deletion on Endpoint regardless of audience
        assert_eq!(
            classify(
                "Endpoint",
                Operation::Delete,
                &json!({ "audience": "internal" })
            ),
            Lane::Red
        );
        assert_eq!(
            classify(
                "Endpoint",
                Operation::Delete,
                &json!({ "audience": "public" })
            ),
            Lane::Red
        );
    }

    #[test]
    fn endpoint_audience_classification() {
        let public_spec = json!({ "audience": "public" });
        assert_eq!(
            classify("Endpoint", Operation::Create, &public_spec),
            Lane::Red
        );
        assert_eq!(
            classify("Endpoint", Operation::Update, &public_spec),
            Lane::Red
        );

        let internal_spec = json!({ "audience": "internal" });
        assert_eq!(
            classify("Endpoint", Operation::Create, &internal_spec),
            Lane::Yellow
        );
        assert_eq!(
            classify("Endpoint", Operation::Update, &internal_spec),
            Lane::Yellow
        );

        let omitted_audience = json!({});
        assert_eq!(
            classify("Endpoint", Operation::Create, &omitted_audience),
            Lane::Yellow
        );
    }

    #[test]
    fn federation_and_dataspace_edges_are_red() {
        let kinds = [
            "ContextSourceRegistration",
            "SharedSpaceReference",
            "DataSpaceParticipant",
            "DataOffer",
            "DataAgreement",
        ];
        let spec = json!({});
        for kind in kinds {
            assert_eq!(
                classify(kind, Operation::Create, &spec),
                Lane::Red,
                "{kind} create must be Red"
            );
            assert_eq!(
                classify(kind, Operation::Update, &spec),
                Lane::Red,
                "{kind} update must be Red"
            );
        }
    }

    #[test]
    fn identity_and_access_kinds_are_red() {
        let kinds = [
            "ServiceAccount",
            "Policy",
            "ScopeDefinition",
            "Organization",
            "Project",
        ];
        let spec = json!({});
        for kind in kinds {
            assert_eq!(
                classify(kind, Operation::Create, &spec),
                Lane::Red,
                "{kind} create must be Red"
            );
            assert_eq!(
                classify(kind, Operation::Update, &spec),
                Lane::Red,
                "{kind} update must be Red"
            );
        }
    }

    #[test]
    fn context_space_sandbox_is_green_while_regular_space_is_yellow() {
        let sandbox = json!({ "isSandbox": true });
        assert_eq!(
            classify("ContextSpace", Operation::Create, &sandbox),
            Lane::Green
        );
        assert_eq!(
            classify("ContextSpace", Operation::Update, &sandbox),
            Lane::Green
        );

        let regular = json!({ "isSandbox": false });
        assert_eq!(
            classify("ContextSpace", Operation::Create, &regular),
            Lane::Yellow
        );
        assert_eq!(
            classify("ContextSpace", Operation::Update, &regular),
            Lane::Yellow
        );

        let omitted = json!({});
        assert_eq!(
            classify("ContextSpace", Operation::Create, &omitted),
            Lane::Yellow
        );
    }

    #[test]
    fn dashboards_and_layers_are_green() {
        let spec = json!({});
        assert_eq!(classify("Dashboard", Operation::Create, &spec), Lane::Green);
        assert_eq!(classify("Dashboard", Operation::Update, &spec), Lane::Green);
        assert_eq!(classify("Layer", Operation::Create, &spec), Lane::Green);
        assert_eq!(classify("Layer", Operation::Update, &spec), Lane::Green);
    }

    #[test]
    fn everything_else_is_yellow() {
        let spec = json!({});
        let kinds = [
            "DataModel",
            "Pipeline",
            "App",
            "Subscription",
            "Entity",
            "Mapping",
            "SyncSource",
            "Blueprint",
            "UnknownResourceKind",
        ];
        for kind in kinds {
            assert_eq!(
                classify(kind, Operation::Create, &spec),
                Lane::Yellow,
                "{kind} create must be Yellow"
            );
            assert_eq!(
                classify(kind, Operation::Update, &spec),
                Lane::Yellow,
                "{kind} update must be Yellow"
            );
        }
    }

    #[test]
    fn change_meta_name_formatting() {
        let meta = ChangeMeta::from_merge_request(42, "ovzdusie");
        assert_eq!(meta.name, "chg-0000002a");
        assert_eq!(meta.namespace, "ovzdusie");

        let from_str = ChangeMeta::from_mr_str("https://git.example.sk/pulls/1", "default");
        assert_eq!(from_str.name, "chg-00000001");

        let from_raw = ChangeMeta::from_mr_str("branch-custom-ref", "default");
        assert!(from_raw.name.starts_with("chg-"));
        assert_eq!(from_raw.name.len(), 12); // "chg-" (4) + 8 hex digits
    }

    #[test]
    fn change_json_serialization() {
        let change = Change::new(
            ChangeMeta::from_merge_request(1, "ovzdusie"),
            ChangeStatus::new(
                Lane::Green,
                ChangePhase::PendingApproval,
                PlanSummary::new(1, 0, 0),
            )
            .with_merge_request("https://git.example.sk/pulls/1"),
        );

        let serialized = serde_json::to_value(&change).expect("serialize change");
        assert_eq!(serialized["apiVersion"], "joinedcontext.com/v1alpha1");
        assert_eq!(serialized["kind"], "Change");
        assert_eq!(serialized["metadata"]["name"], "chg-00000001");
        assert_eq!(serialized["metadata"]["namespace"], "ovzdusie");
        assert_eq!(serialized["status"]["lane"], "green");
        assert_eq!(serialized["status"]["phase"], "PendingApproval");
        assert_eq!(
            serialized["status"]["mergeRequest"],
            "https://git.example.sk/pulls/1"
        );
        assert_eq!(serialized["status"]["plan"]["create"], 1);
        assert_eq!(serialized["status"]["plan"]["update"], 0);
        assert_eq!(serialized["status"]["plan"]["delete"], 0);
    }
}
