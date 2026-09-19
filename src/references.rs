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
    /// Where this reference is written in the manifest being checked, e.g. `spec.dataModelRef` or
    /// `spec.sources[0].dataSourceRef`. The same spelling the dry run's own `trace` uses for the
    /// same field, so a finding and the plan beside it never name one field two ways — including
    /// after `pipeline_second_shape` has rewritten a `spec.source` into `spec.sources[0]` (PL-54).
    path: String,
    kind: &'a str,
    name: &'a str,
    /// The project the reference names, when it names one other than the manifest's own.
    namespace: Option<&'a str>,
}

/// The name a reference holds, whether it is written as a bare string or as a typed reference.
///
/// Both spellings are accepted by the kinds (`Ref`), so both are resolved: a manifest committed
/// before the typed form existed still points at a real resource or at nothing.
fn named<'a>(value: &'a Value, path: String, default_kind: &'a str) -> Option<Reference<'a>> {
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

/// Every kind whose references are resolved here, beside the table below: a kind in one and not
/// the other fails `every_path_named_here_exists_in_that_kinds_schema`, which is the only reader.
#[cfg(test)]
const KINDS: &[&str] = &[
    "ContextSpace",
    "ModelProjection",
    "Endpoint",
    "Mapping",
    "Pipeline",
    "Subscription",
    "Dashboard",
    "App",
];

/// The fields of one kind that name a resource of this platform, in this project, with the kind a
/// bare name means.
///
/// Every path here is a field the kind's own spec really carries, and the guard test fails on one
/// that does not: four entries once named fields no spec had (`Pipeline.spec.dataSourceRef` among
/// them), so three kinds were checked for nothing at all while the table looked complete (T-2268).
///
/// `[]` stands for every element of an array. A field whose door has its own resolution rule is
/// absent on purpose and says so beside it.
fn fields(kind: &str) -> &'static [(&'static str, &'static str)] {
    match kind {
        "ContextSpace" => &[("spec.dataModelRef", "DataModel")],
        "ModelProjection" => &[
            ("spec.dataModelRef", "DataModel"),
            ("spec.contextSpaceRef", "ContextSpace"),
        ],
        // `spec.policyRef` is an NGSI-LD URN rather than a name in this project, so resolving it
        // means parsing it; it is left to the door that reads policies.
        "Endpoint" => &[
            ("spec.contextSpaceRef", "ContextSpace"),
            ("spec.projectionRef", "ModelProjection"),
            ("spec.viewMappingRef", "Mapping"),
        ],
        "Mapping" => &[("spec.contextSpaceRef", "ContextSpace")],
        // Both shapes of a pipeline (PL-54): the door rewrites `source`/`compute` into
        // `sources`/`steps` only when a `targetEndpoint` is there too, so a manifest can still
        // arrive in either. `spec.outputs[].targetEndpoint` is a URN, not a name, and is left out
        // for the same reason as `Endpoint.spec.policyRef`.
        "Pipeline" => &[
            ("spec.source.dataSourceRef", "DataSource"),
            ("spec.source.endpointRef", "Endpoint"),
            ("spec.sources[].dataSourceRef", "DataSource"),
            ("spec.sources[].endpointRef", "Endpoint"),
            ("spec.compute.mappingRef", "Mapping"),
            ("spec.steps[].mappingRef", "Mapping"),
        ],
        "Subscription" => &[("spec.contextSpaceRef", "ContextSpace")],
        // A dashboard names no space of its own: it reads through the endpoints its widgets name.
        // Its `spec.pages[].layers[]` are deliberately absent, and so is a `Layer`'s own
        // `spec.sourceEndpointRef`: a dashboard is drafted **with** its layers, and the endpoint
        // they read is drafted beside them (`jc_space_complete`, `change_resource`), so nothing of
        // that set is in the mirror yet and this check would refuse the ordinary way a map is made.
        // Those two are resolved where the batch is known: `tools_change` refuses a page drawing a
        // layer that is neither the project's nor one of the new ones, and `dashboards::check`
        // resolves the layers of a public dashboard together with their audience (UI-19).
        "Dashboard" => &[("spec.pages[].widgets[].endpointRef", "Endpoint")],
        "App" => &[("spec.dataNeeds[].contextSpaceRef", "ContextSpace")],
        // A `DataSource` names no resource of this platform — its `secretRef`s are resolved by the
        // reconciler, not by the mirror. `SharedSpaceReference.endpointRef` names a resource of
        // another organization and is resolved by what the Endpoint admits (T-1448).
        _ => &[],
    }
}

/// Every value a dotted path addresses, with the path each one is written at.
///
/// A path is walked segment by segment because a reference is not always a field of the spec: a
/// pipeline's data source lives under `spec.source`, a dashboard's endpoints under
/// `spec.pages[].widgets[]`. `[]` reads every element and the path carries its index, so a finding
/// names the one widget that is wrong. A segment the manifest does not have, or an array where the
/// path expects one, yields nothing: an optional reference left out is not a finding.
fn at<'a>(value: &'a Value, segments: &[&str], path: String, found: &mut Vec<(String, &'a Value)>) {
    let Some((head, rest)) = segments.split_first() else {
        found.push((path, value));
        return;
    };
    match head.strip_suffix("[]") {
        Some(field) => {
            let items = value.get(field).and_then(Value::as_array);
            for (index, item) in items.into_iter().flatten().enumerate() {
                at(item, rest, format!("{path}.{field}[{index}]"), found);
            }
        }
        None => {
            if let Some(next) = value.get(head) {
                at(next, rest, format!("{path}.{head}"), found);
            }
        }
    }
}

/// The references of one kind's spec, each with the path it is written at.
fn references<'a>(kind: &str, spec: &'a Value) -> Vec<Reference<'a>> {
    let mut references = Vec::new();
    for (path, default_kind) in fields(kind) {
        let segments: Vec<&str> = path.trim_start_matches("spec.").split('.').collect();
        let mut values = Vec::new();
        at(spec, &segments, "spec".to_owned(), &mut values);
        references.extend(
            values
                .into_iter()
                .filter_map(|(written_at, value)| named(value, written_at, default_kind)),
        );
    }
    references
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
                errors: vec![reference.path],
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

    /// The field and the name a refusal carries, for the cases below.
    fn refusal(mirror: &Mirror, kind: &str, spec: Value) -> (String, Vec<String>) {
        let refused = check(mirror, "helsinki", kind, &spec)
            .expect_err("a reference that is not there is refused");
        let ApiError::Invalid { detail, errors } = refused else {
            panic!("a missing reference is an invalid manifest, not {refused:?}");
        };
        (detail, errors)
    }

    /// T-2268: a pipeline's data source does not live at `spec.dataSourceRef` — it lives one level
    /// down, which is why this was checked green and only the reconciler found out.
    #[test]
    fn a_pipeline_naming_a_data_source_that_is_not_there_is_refused_by_its_field() {
        let mirror = mirror_with("DataSource", "gbfs");
        let (detail, errors) = refusal(
            &mirror,
            "Pipeline",
            json!({
                "class": "auto",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "not-there-at-all" } }
            }),
        );
        assert_eq!(errors, vec!["spec.source.dataSourceRef".to_owned()]);
        assert!(detail.contains("not-there-at-all"), "{detail}");

        check(
            &mirror,
            "helsinki",
            "Pipeline",
            &json!({ "class": "auto", "source": { "dataSourceRef": "gbfs" } }),
        )
        .expect("a data source that is there resolves");
    }

    /// The second shape (PL-54) is the one the door stores, and a pipeline may have several inputs:
    /// the finding names the element that is wrong, not the field in general.
    #[test]
    fn a_pipeline_with_several_sources_names_the_one_that_is_missing() {
        let mirror = mirror_with("DataSource", "gbfs");
        let (_, errors) = refusal(
            &mirror,
            "Pipeline",
            json!({
                "class": "auto",
                "sources": [
                    { "dataSourceRef": "gbfs" },
                    { "dataSourceRef": "absent" }
                ]
            }),
        );
        assert_eq!(errors, vec!["spec.sources[1].dataSourceRef".to_owned()]);
    }

    /// A pipeline reads the platform's own spaces through an endpoint, and computes through a
    /// mapping: both are references and neither was resolved.
    #[test]
    fn a_pipelines_source_endpoint_and_its_mapping_are_resolved_too() {
        let mirror = Mirror::new();
        let (_, errors) = refusal(
            &mirror,
            "Pipeline",
            json!({ "class": "auto", "source": { "endpointRef": "no-such-endpoint" } }),
        );
        assert_eq!(errors, vec!["spec.source.endpointRef".to_owned()]);

        let (detail, errors) = refusal(
            &mirror,
            "Pipeline",
            json!({
                "class": "auto",
                "steps": [{ "kind": "mapping", "mappingRef": "no-such-mapping" }]
            }),
        );
        assert_eq!(errors, vec!["spec.steps[0].mappingRef".to_owned()]);
        assert!(detail.contains("Mapping"), "{detail}");
    }

    /// A dashboard names no space: it names the endpoints its widgets read and the layers its
    /// pages draw, both nested and repeated, and none of them used to be resolved.
    #[test]
    fn a_dashboard_widget_naming_an_endpoint_that_is_not_there_is_refused_by_its_own_element() {
        let mirror = mirror_with("Endpoint", "bikes-public");
        let (_, errors) = refusal(
            &mirror,
            "Dashboard",
            json!({
                "title": "Bikes",
                "pages": [
                    { "widgets": [{ "widgetType": "value", "endpointRef": "bikes-public" }] },
                    { "widgets": [
                        { "widgetType": "value", "endpointRef": "bikes-public" },
                        { "widgetType": "value", "endpointRef": "gone" }
                    ] }
                ]
            }),
        );
        assert_eq!(
            errors,
            vec!["spec.pages[1].widgets[1].endpointRef".to_owned()]
        );

        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "Layer".to_owned(),
            metadata: ObjectMeta {
                name: "stations".to_owned(),
                namespace: Some("helsinki".to_owned()),
                ..ObjectMeta::default()
            },
            spec: json!({}),
            status: None,
        });
        // A page's layers are not resolved here: a dashboard is drafted with its layers, so at
        // check time they are drafts and not manifests of the project (see `fields`).
        check(
            &mirror,
            "helsinki",
            "Dashboard",
            &json!({ "title": "Bikes", "pages": [{ "layers": ["stations", "not-committed-yet"] }] }),
        )
        .expect("the layers of a new dashboard are judged where the batch is known");
    }

    /// An app declares the spaces it needs by name, in its own project, so they are resolved here.
    /// A `Layer`'s endpoint is not: it is drafted with the dashboard that draws it.
    #[test]
    fn an_apps_data_need_is_resolved_and_a_layers_endpoint_is_not() {
        let mirror = Mirror::new();
        check(
            &mirror,
            "helsinki",
            "Layer",
            &json!({ "entityType": "GtfsStop", "sourceEndpointRef": "drafted-beside-it" }),
        )
        .expect("a layer's endpoint is judged where the batch is known");

        let (_, errors) = refusal(
            &mirror,
            "App",
            json!({ "dataNeeds": [{ "contextSpaceRef": "no-such-space", "types": ["GtfsStop"] }] }),
        );
        assert_eq!(errors, vec!["spec.dataNeeds[0].contextSpaceRef".to_owned()]);
    }

    /// An array the path expects and does not find is not a finding, and neither is an element
    /// without the reference: a manifest half typed is incomplete, not wrong here.
    #[test]
    fn a_path_the_manifest_does_not_have_is_not_a_finding() {
        let mirror = Mirror::new();
        for spec in [
            json!({ "class": "auto" }),
            json!({ "class": "auto", "sources": "not-an-array" }),
            json!({ "class": "auto", "sources": [{}, { "query": { "type": "GtfsStop" } }] }),
            json!({ "class": "auto", "source": { "dataSourceRef": null } }),
        ] {
            check(&mirror, "helsinki", "Pipeline", &spec)
                .unwrap_or_else(|error| panic!("{spec} is incomplete, not wrong: {error:?}"));
        }
    }

    /// The guard on the table itself (T-2268): four entries once named fields no spec carried, so
    /// three kinds were checked for nothing while the table looked complete. A path that is not a
    /// field of that kind's spec fails here instead of being discovered on the cluster.
    #[test]
    fn every_path_named_here_exists_in_that_kinds_schema() {
        for kind in KINDS {
            let schema = spec_schema(kind);
            assert!(
                !fields(kind).is_empty(),
                "{kind} is listed and names nothing"
            );
            for (path, named) in fields(kind) {
                assert!(
                    has_path(&schema, path),
                    "{kind}: {path} is not a field of {kind}Spec, so {named} is never resolved"
                );
            }
        }
        // The guard can fail: these are the four paths this task removed.
        for (kind, dead) in [
            ("Pipeline", "spec.dataSourceRef"),
            ("Pipeline", "spec.contextSpaceRef"),
            ("Dashboard", "spec.contextSpaceRef"),
            ("Subscription", "spec.notification.contextSpaceRef"),
        ] {
            assert!(
                !has_path(&spec_schema(kind), dead),
                "{kind}Spec has no {dead}; the guard has stopped seeing that"
            );
        }
    }

    /// One kind's spec as `jc-core` renders it, for the guard above.
    fn spec_schema(kind: &str) -> schemars::schema::RootSchema {
        use jc_core::kinds::*;
        match kind {
            "ContextSpace" => schemars::schema_for!(ContextSpaceSpec),
            "ModelProjection" => schemars::schema_for!(ModelProjectionSpec),
            "Endpoint" => schemars::schema_for!(EndpointSpec),
            "Mapping" => schemars::schema_for!(MappingSpec),
            "Pipeline" => schemars::schema_for!(PipelineSpec),
            "Subscription" => schemars::schema_for!(SubscriptionSpec),
            "Dashboard" => schemars::schema_for!(DashboardSpec),
            "Layer" => schemars::schema_for!(LayerSpec),
            "App" => schemars::schema_for!(AppSpec),
            other => panic!("{other} is in KINDS with no schema beside it"),
        }
    }

    /// Every object a field's schema can be: a `$ref` followed, and each branch of an `anyOf` or
    /// `allOf` kept, because a pipeline `Step` is either a processor or a compute kind and a
    /// reference lives in only one of the two. For a guard on field names, "any branch has it" is
    /// the question being asked.
    fn branches<'a>(
        schema: &'a schemars::schema::Schema,
        root: &'a schemars::schema::RootSchema,
    ) -> Vec<&'a schemars::schema::SchemaObject> {
        let schemars::schema::Schema::Object(object) = schema else {
            return Vec::new();
        };
        if let Some(reference) = &object.reference {
            let name = reference.trim_start_matches("#/definitions/");
            return root
                .definitions
                .get(name)
                .map(|named| branches(named, root))
                .unwrap_or_default();
        }
        if let Some(subschemas) = &object.subschemas {
            let nested: Vec<_> = subschemas
                .any_of
                .iter()
                .chain(subschemas.all_of.iter())
                .flatten()
                .flat_map(|branch| branches(branch, root))
                .collect();
            if !nested.is_empty() {
                return nested;
            }
        }
        vec![object]
    }

    /// Whether a dotted path, `[]` and all, addresses a field of this spec.
    fn has_path(root: &schemars::schema::RootSchema, path: &str) -> bool {
        let segments: Vec<&str> = path.trim_start_matches("spec.").split('.').collect();
        holds(&root.schema, &segments, root)
    }

    fn holds(
        at: &schemars::schema::SchemaObject,
        segments: &[&str],
        root: &schemars::schema::RootSchema,
    ) -> bool {
        use schemars::schema::SingleOrVec;
        let Some((segment, rest)) = segments.split_first() else {
            return true;
        };
        let repeated = segment.ends_with("[]");
        let field = segment.trim_end_matches("[]");
        let Some(property) = at
            .object
            .as_ref()
            .and_then(|object| object.properties.get(field))
        else {
            return false;
        };
        if !repeated && rest.is_empty() {
            return true;
        }
        branches(property, root).into_iter().any(|object| {
            if !repeated {
                return holds(object, rest, root);
            }
            let Some(items) = object.array.as_ref().and_then(|array| array.items.as_ref()) else {
                return false;
            };
            if rest.is_empty() {
                return true;
            }
            let elements: Vec<_> = match items {
                SingleOrVec::Single(schema) => vec![schema.as_ref()],
                SingleOrVec::Vec(schemas) => schemas.iter().collect(),
            };
            elements.into_iter().any(|element| {
                branches(element, root)
                    .into_iter()
                    .any(|element| holds(element, rest, root))
            })
        })
    }
}
