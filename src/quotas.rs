//! What a project may hold, and what it holds now (PF-73, PF-74, PF-75).
//!
//! The organization sets the default quota of every project it opens; a project may carry its
//! own. The Portal counts what the repository declares — not what a cluster reports — because
//! the refusal has to happen before a `Change` exists, on every door (PF-74).
//!
//! Four dimensions are countable from the manifests: context spaces, resident pipelines, public
//! endpoints and apps. The rest (`ingestEventsPerSecond`, `entitiesPerSpace`,
//! `requestsPerMinute`, `agentRunsPerDay`) are runtime limits enforced where they happen.

use std::collections::BTreeMap;

use jc_core::kinds::space::Quotas;
use serde_json::Value;

use crate::error::ApiError;
use crate::permissions::ORG_NAMESPACE;
use crate::store::{ListOptions, Mirror};

/// The dimensions a manifest count answers, and the kind each one counts.
const COUNTED: [(&str, &str); 4] = [
    ("contextSpaces", "ContextSpace"),
    ("residentPipelines", "Pipeline"),
    ("publicEndpoints", "Endpoint"),
    ("apps", "App"),
];

/// The quota in force for `project`: its own `spec.quotas` when it has one, else the
/// organization's `spec.projects.quota`, else no limit at all (PF-73).
pub fn effective(mirror: &Mirror, project: &str) -> Quotas {
    let own = mirror
        .get(ORG_NAMESPACE, "Project", project)
        .and_then(|env| serde_json::from_value::<Quotas>(env.spec.get("quotas")?.clone()).ok());
    own.unwrap_or_else(|| organization_default(mirror))
}

/// The default every project of the organization starts from (PF-73).
pub fn organization_default(mirror: &Mirror) -> Quotas {
    mirror
        .list(ORG_NAMESPACE, "Organization", &ListOptions::default())
        .items
        .into_iter()
        .find_map(|env| {
            serde_json::from_value::<Quotas>(env.spec.pointer("/projects/quota")?.clone()).ok()
        })
        .unwrap_or_default()
}

/// Whether one manifest counts against `dimension`: every context space and app, a pipeline the
/// runner keeps resident, an endpoint open to the public.
fn counts(dimension: &str, kind: &str, spec: &Value) -> bool {
    match dimension {
        "contextSpaces" => kind == "ContextSpace",
        "apps" => kind == "App",
        "publicEndpoints" => {
            kind == "Endpoint" && spec.get("audience").and_then(Value::as_str) == Some("public")
        }
        "residentPipelines" => {
            kind == "Pipeline"
                && serde_json::from_value::<jc_core::kinds::pipeline::PipelineSpec>(spec.clone())
                    .map(|spec| crate::reconciler::streams::is_stream_pipeline(&spec))
                    .unwrap_or(false)
        }
        _ => false,
    }
}

/// What the project holds now, per countable dimension (PF-75).
pub fn usage(mirror: &Mirror, project: &str) -> BTreeMap<String, u32> {
    COUNTED
        .into_iter()
        .map(|(dimension, kind)| {
            let held = mirror
                .list(project, kind, &ListOptions::default())
                .items
                .into_iter()
                .filter(|env| counts(dimension, kind, &env.spec))
                .count() as u32;
            (dimension.to_owned(), held)
        })
        .collect()
}

/// The limit of each countable dimension, absent where the quota leaves it open.
pub fn limits(quotas: &Quotas) -> BTreeMap<String, u32> {
    quotas
        .dimensions()
        .into_iter()
        .filter(|(dimension, _)| COUNTED.iter().any(|(counted, _)| counted == dimension))
        .filter_map(|(dimension, limit)| limit.map(|limit| (dimension.to_owned(), limit)))
        .collect()
}

/// Refuses the write that would put the project over one of its quotas, naming the limit and the
/// count (PF-74). `name` is the manifest being written, so replacing what is already counted is
/// not counted twice; a deletion never exceeds anything.
pub fn check(
    mirror: &Mirror,
    project: &str,
    kind: &str,
    name: &str,
    spec: &Value,
) -> Result<(), ApiError> {
    let quotas = effective(mirror, project);
    for (dimension, counted_kind) in COUNTED {
        if counted_kind != kind || !counts(dimension, kind, spec) {
            continue;
        }
        let Some(limit) = limits(&quotas).get(dimension).copied() else {
            continue;
        };
        let others = mirror
            .list(project, kind, &ListOptions::default())
            .items
            .into_iter()
            .filter(|env| env.metadata.name != name && counts(dimension, kind, &env.spec))
            .count() as u32;
        let after = others + 1;
        if after > limit {
            return Err(ApiError::Denied(format!(
                "quota: {dimension} {after} of {limit} in project {project}; raise the quota on \
                 the project or the organization first (PF-73, PF-74)"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::json;

    fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    fn resident(name: &str) -> ResourceEnvelope {
        envelope(
            "Pipeline",
            name,
            "ovzdusie",
            json!({
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:bb.sk:ovzdusie:air"
            }),
        )
    }

    fn mirror_with(items: Vec<ResourceEnvelope>) -> Mirror {
        let mirror = Mirror::new();
        for item in items {
            mirror.upsert(item);
        }
        mirror
    }

    #[test]
    fn the_projects_own_quota_wins_over_the_organizations_default() {
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "bb",
                ORG_NAMESPACE,
                json!({ "domain": "bb.sk", "projects": { "quota": { "residentPipelines": 3 } } }),
            ),
            envelope(
                "Project",
                "ovzdusie",
                ORG_NAMESPACE,
                json!({ "organizationRef": { "name": "bb" }, "quotas": { "residentPipelines": 1 } }),
            ),
        ]);
        assert_eq!(effective(&mirror, "ovzdusie").resident_pipelines, Some(1));
        assert_eq!(effective(&mirror, "doprava").resident_pipelines, Some(3));
    }

    #[test]
    fn the_next_one_over_the_limit_is_refused_and_the_message_names_both_numbers() {
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "bb",
                ORG_NAMESPACE,
                json!({ "domain": "bb.sk", "projects": { "quota": { "residentPipelines": 2 } } }),
            ),
            resident("one"),
            resident("two"),
        ]);
        assert_eq!(
            usage(&mirror, "ovzdusie").get("residentPipelines"),
            Some(&2)
        );

        let refused = check(
            &mirror,
            "ovzdusie",
            "Pipeline",
            "three",
            &resident("three").spec,
        )
        .expect_err("the third resident pipeline");
        assert!(
            refused.to_string().contains("residentPipelines 3 of 2"),
            "{refused}"
        );

        // Writing one of the two again is not a third: the same name replaces itself.
        check(
            &mirror,
            "ovzdusie",
            "Pipeline",
            "two",
            &resident("two").spec,
        )
        .expect("an update of what is already counted");
    }

    #[test]
    fn a_dimension_nobody_limited_never_refuses_anything() {
        let mirror = mirror_with(vec![resident("one"), resident("two"), resident("three")]);
        check(
            &mirror,
            "ovzdusie",
            "Pipeline",
            "four",
            &resident("four").spec,
        )
        .expect("no quota, no limit");
    }

    #[test]
    fn only_a_public_endpoint_counts_against_the_public_endpoint_quota() {
        let mirror = mirror_with(vec![
            envelope(
                "Organization",
                "bb",
                ORG_NAMESPACE,
                json!({ "domain": "bb.sk", "projects": { "quota": { "publicEndpoints": 1 } } }),
            ),
            envelope(
                "Endpoint",
                "air",
                "ovzdusie",
                json!({ "contextSpaceRef": "ovzdusie", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y", "audience": "public" }),
            ),
        ]);
        // An internal one is not counted, however many there are.
        check(
            &mirror,
            "ovzdusie",
            "Endpoint",
            "traffic",
            &json!({ "contextSpaceRef": "ovzdusie", "audience": "organization" }),
        )
        .expect("an internal endpoint");

        let refused = check(
            &mirror,
            "ovzdusie",
            "Endpoint",
            "traffic",
            &json!({ "contextSpaceRef": "ovzdusie", "audience": "public" }),
        )
        .expect_err("the second public endpoint");
        assert!(
            refused.to_string().contains("publicEndpoints 2 of 1"),
            "{refused}"
        );
    }
}
