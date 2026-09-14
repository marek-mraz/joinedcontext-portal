//! What an agent may call (AG-70): the profile's `spec.access` narrows the operation registry,
//! and the person who started the run narrows it again at every call, so a profile never widens
//! anyone. Read from the untyped spec like the rest of the profile (see `profile.rs`).

use std::collections::{BTreeMap, BTreeSet};

use jc_core::kinds::Verb;
use serde_json::Value;

use crate::auth::session::Identity;
use crate::ops::{self, Operation};
use crate::state::AppState;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    /// `None` when the profile has no access block: operations annotated read-only, nothing else.
    declared: Option<Declared>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Declared {
    operations: BTreeSet<String>,
    /// Kind → the verbs granted on it (`read`, `propose`).
    kinds: BTreeMap<String, BTreeSet<String>>,
}

impl Access {
    pub fn from_spec(spec: &Value) -> Self {
        let Some(access) = spec.get("access").filter(|access| access.is_object()) else {
            return Self::default();
        };
        let strings = |value: Option<&Value>| -> BTreeSet<String> {
            value
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let kinds = access
            .get("kinds")
            .and_then(Value::as_array)
            .map(|grants| {
                grants
                    .iter()
                    .filter_map(|grant| {
                        let kind = grant.get("kind")?.as_str()?.to_owned();
                        Some((kind, strings(grant.get("verbs"))))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            declared: Some(Declared {
                operations: strings(access.get("operations")),
                kinds,
            }),
        }
    }

    /// The profile's half. Without an access block: an operation annotated read-only. With one:
    /// an operation it names whose kind verb it grants, `read` for a verbless operation of a
    /// kind and `propose` for a proposal; approving and deleting are never an agent's.
    pub fn names(&self, op: &Operation) -> bool {
        let Some(declared) = &self.declared else {
            return op.annotations.read_only_hint;
        };
        if !declared.operations.contains(op.name) {
            return false;
        }
        if op.kind == "*" {
            return true;
        }
        let verb = match op.verb {
            None => "read",
            Some(Verb::Propose) => "propose",
            Some(_) => return false,
        };
        declared
            .kinds
            .get(op.kind)
            .is_some_and(|verbs| verbs.contains(verb))
    }

    /// Both halves at the moment of the call: the profile names the operation and the person who
    /// started the run may call it. The error is the reason the refused `tool` event carries.
    pub fn check(
        &self,
        name: &str,
        identity: &Identity,
        state: &AppState,
        project: &str,
    ) -> Result<(), String> {
        let op = ops::find(name).ok_or_else(|| format!("operation '{name}' is not registered"))?;
        if !self.names(op) {
            return Err(format!("the agent profile does not grant {name} (AG-70)"));
        }
        ops::permitted(op, identity, state, project).map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(name: &str) -> &'static Operation {
        ops::find(name).expect("registered")
    }

    #[test]
    fn no_access_block_allows_read_only_operations_only() {
        let access = Access::from_spec(&json!({ "role": "builder" }));
        assert!(access.names(op("jc_catalog_search")));
        assert!(access.names(op("jc_kpi_compute")));
        assert!(!access.names(op("jc_space_complete")));
        assert!(!access.names(op("jc_change_approve")));
    }

    #[test]
    fn a_declared_operation_needs_its_kind_verb() {
        let access = Access::from_spec(&json!({ "access": {
            "operations": ["jc_catalog_search", "jc_endpoint_propose", "jc_kpi_compute", "jc_change_approve"],
            "kinds": [{ "kind": "Endpoint", "verbs": ["read"] }, { "kind": "Change", "verbs": ["read", "propose"] }],
        }}));
        assert!(access.names(op("jc_catalog_search")));
        assert!(access.names(op("jc_kpi_compute")));
        assert!(
            !access.names(op("jc_endpoint_propose")),
            "read does not grant propose"
        );
        assert!(
            !access.names(op("jc_change_approve")),
            "approve is never an agent's"
        );
        assert!(!access.names(op("jc_space_complete")), "not named");
        assert!(
            !access.names(op("jc_manifest_dry_run")),
            "read-only but not named"
        );
    }
}
