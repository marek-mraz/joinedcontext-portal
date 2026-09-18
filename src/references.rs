//! Every resource a manifest names has to be there (MF-13, PF-57).
//!
//! A manifest points at other resources by name: a space at its `DataModel`, an endpoint at its
//! `ModelProjection`, a mapping at its SSSOM document. Until this, the check resolved none of
//! them: a space whose `spec.dataModelRef` named a model that did not exist was checked green,
//! proposable and approvable, and only the reconciler found out — away from the person who could
//! have fixed it in the form (T-2233).
//!
//! The rule is the one `spaces::check` already follows: one function, called by the dry run of
//! every door, refusing with the field and the name it could not find. A reference into another
//! project is not resolved here — that door has its own rule, and `SharedSpaceReference` resolves
//! its `endpointRef` against what the Endpoint admits (T-1448) rather than against the mirror.

use crate::error::ApiError;
use crate::store::Mirror;
use serde_json::Value;

/// One reference a manifest carries: the field that holds it and the resource it names.
struct Reference<'a> {
    /// The JSON path a person reads in the form, e.g. `spec.dataModelRef`.
    path: &'a str,
    kind: &'a str,
    name: &'a str,
    /// The project the reference names, when it names one other than the manifest's own.
    namespace: Option<&'a str>,
}

/// The name a reference holds, whether it is written as a bare string or as a typed reference.
///
/// Both spellings are accepted by the kinds (`Ref`), so both are resolved: a manifest committed
/// before the typed form existed still points at a real resource or at nothing.
fn named<'a>(value: &'a Value, path: &'a str, default_kind: &'a str) -> Option<Reference<'a>> {
    match value {
        Value::String(name) if !name.is_empty() => Some(Reference {
            path,
            kind: default_kind,
            name,
            namespace: None,
        }),
        Value::Object(members) => {
            let name = members.get("name").and_then(Value::as_str)?;
            Some(Reference {
                path,
                kind: members
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or(default_kind),
                name,
                namespace: members.get("namespace").and_then(Value::as_str),
            })
        }
        _ => None,
    }
}

/// The references of one kind's spec, by the field each lives in.
///
/// Only the fields that name a resource of this platform, in this project. A field whose door has
/// its own resolution rule is absent on purpose and says so beside it.
fn references<'a>(kind: &str, spec: &'a Value) -> Vec<Reference<'a>> {
    let one = |path: &'a str, default_kind: &'a str| -> Option<Reference<'a>> {
        spec.pointer(&format!("/{}", path.trim_start_matches("spec.")))
            .and_then(|value| named(value, path, default_kind))
    };
    let fields: &[(&'a str, &'a str)] = match kind {
        "ContextSpace" => &[("spec.dataModelRef", "DataModel")],
        "ModelProjection" => &[("spec.dataModelRef", "DataModel")],
        "Endpoint" => &[
            ("spec.contextSpaceRef", "ContextSpace"),
            ("spec.projectionRef", "ModelProjection"),
            ("spec.viewMappingRef", "Mapping"),
        ],
        "Mapping" => &[("spec.contextSpaceRef", "ContextSpace")],
        "Pipeline" => &[
            ("spec.contextSpaceRef", "ContextSpace"),
            ("spec.dataSourceRef", "DataSource"),
        ],
        "DataSource" => &[("spec.contextSpaceRef", "ContextSpace")],
        "Dashboard" => &[("spec.contextSpaceRef", "ContextSpace")],
        "Subscription" => &[("spec.contextSpaceRef", "ContextSpace")],
        // `SharedSpaceReference.endpointRef` names a resource of another organization and is
        // resolved by what the Endpoint admits, not by this mirror (T-1448).
        _ => &[],
    };
    fields
        .iter()
        .filter_map(|(path, kind)| one(path, kind))
        .collect()
}

/// Refuses a manifest naming a resource that is not there (MF-13).
///
/// Called by the dry run of every door, before a verdict is recorded, so a person meets the
/// missing name in the form they typed it into. A reference into another project is left to the
/// door that owns it: resolving it here would answer for a project the caller may not read.
pub fn check(mirror: &Mirror, project: &str, kind: &str, spec: &Value) -> Result<(), ApiError> {
    for reference in references(kind, spec) {
        if reference
            .namespace
            .is_some_and(|namespace| namespace != project)
        {
            continue;
        }
        if mirror
            .get(project, reference.kind, reference.name)
            .is_none()
        {
            return Err(ApiError::Invalid {
                detail: format!(
                    "{} names {} '{}', which does not exist in project '{}'; propose it first, \
                     or name one that is there",
                    reference.path, reference.kind, reference.name, project
                ),
                errors: vec![reference.path.to_owned()],
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::json;

    fn mirror_with(kind: &str, name: &str) -> Mirror {
        let mirror = Mirror::new();
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta {
                name: name.to_owned(),
                namespace: Some("helsinki".to_owned()),
                ..ObjectMeta::default()
            },
            spec: json!({}),
            status: None,
        });
        mirror
    }

    /// MF-13: the finding names the field a person typed into and the resource it could not find.
    #[test]
    fn a_space_naming_a_model_that_does_not_exist_is_refused_by_the_field() {
        let mirror = mirror_with("DataModel", "helsinki");
        let refused = check(
            &mirror,
            "helsinki",
            "ContextSpace",
            &json!({ "dataModelRef": "no-such-model" }),
        )
        .expect_err("a model that is not there is refused");

        let ApiError::Invalid { detail, errors } = refused else {
            panic!("a missing reference is an invalid manifest, not {refused:?}");
        };
        assert!(detail.contains("spec.dataModelRef"), "{detail}");
        assert!(detail.contains("no-such-model"), "{detail}");
        assert_eq!(errors, vec!["spec.dataModelRef".to_owned()]);
    }

    /// MF-13: a reference that resolves is not a finding.
    #[test]
    fn a_space_naming_a_committed_model_passes() {
        let mirror = mirror_with("DataModel", "helsinki");
        check(
            &mirror,
            "helsinki",
            "ContextSpace",
            &json!({ "dataModelRef": "helsinki" }),
        )
        .expect("a model that is there resolves");
    }

    /// The typed spelling resolves the same way as the bare one, and its `kind` is honoured.
    #[test]
    fn a_typed_reference_resolves_by_its_own_kind_and_name() {
        let mirror = mirror_with("DataModel", "helsinki");
        check(
            &mirror,
            "helsinki",
            "ContextSpace",
            &json!({ "dataModelRef": { "kind": "DataModel", "name": "helsinki" } }),
        )
        .expect("the typed spelling resolves");

        check(
            &mirror,
            "helsinki",
            "ContextSpace",
            &json!({ "dataModelRef": { "kind": "DataModel", "name": "absent" } }),
        )
        .expect_err("the typed spelling is resolved too");
    }

    /// An optional reference nobody filled in is not a finding: the field is absent, not wrong.
    #[test]
    fn an_optional_reference_left_out_is_not_a_finding() {
        let mirror = Mirror::new();
        check(&mirror, "helsinki", "ContextSpace", &json!({})).expect("absent is not wrong");
        check(
            &mirror,
            "helsinki",
            "ContextSpace",
            &json!({ "dataModelRef": "" }),
        )
        .expect("an empty string is the field left blank");
    }

    /// A reference that names another project is left to the door that owns that rule (T-1448),
    /// so this check does not answer for a project the caller may not read.
    #[test]
    fn a_reference_into_another_project_is_not_resolved_here() {
        let mirror = Mirror::new();
        check(
            &mirror,
            "helsinki",
            "ContextSpace",
            &json!({ "dataModelRef": { "kind": "DataModel", "name": "shared", "namespace": "tampere" } }),
        )
        .expect("another project's reference is that door's to resolve");
    }

    /// Every reference of a kind is checked, not only the first: an endpoint names three.
    #[test]
    fn an_endpoint_is_refused_for_whichever_reference_is_missing() {
        let mirror = Mirror::new();
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "ContextSpace".to_owned(),
            metadata: ObjectMeta {
                name: "bikes".to_owned(),
                namespace: Some("helsinki".to_owned()),
                ..ObjectMeta::default()
            },
            spec: json!({}),
            status: None,
        });
        let refused = check(
            &mirror,
            "helsinki",
            "Endpoint",
            &json!({ "contextSpaceRef": "bikes", "projectionRef": "no-such-view" }),
        )
        .expect_err("the second reference is checked as well as the first");
        let ApiError::Invalid { detail, .. } = refused else {
            panic!("expected an invalid manifest");
        };
        assert!(detail.contains("spec.projectionRef"), "{detail}");
        assert!(detail.contains("ModelProjection"), "{detail}");
    }

    /// A kind that names nothing resolvable here is left alone, whatever its spec holds.
    #[test]
    fn a_kind_with_no_local_references_is_never_refused() {
        let mirror = Mirror::new();
        check(
            &mirror,
            "helsinki",
            "SharedSpaceReference",
            &json!({ "endpointRef": { "project": "tampere", "name": "bikes" } }),
        )
        .expect("that door resolves its own reference");
    }
}
