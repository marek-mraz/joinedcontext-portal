//! The assistant drafts a publication (EP-72, API/01 §19): from "share the bike stations with
//! the transport team, hide the maintenance notes" to a rendered `Endpoint` manifest with a slug
//! the Portal mints, the draft `Policy` manifests for the audience, and the lane the Change
//! would take. Nothing here writes: the person submits the form the rendering prefills.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;

use crate::change::{self, Lane, Operation};
use crate::resource::{is_dns1123, API_VERSION};

/// The representations an Endpoint may serve (jc-core `Representation`).
pub const REPRESENTATIONS: [&str; 9] = [
    "ngsi-ld",
    "mcp",
    "geojson",
    "csv",
    "xlsx",
    "json",
    "zip",
    "ogc-features",
    "sta",
];
const AUDIENCES: [&str; 3] = ["project-list", "organization", "public"];
const SLUG_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// What the tool takes: the model's or a caller's reading of the request. Unknown fields are
/// ignored and absent ones default, because a model answer is not a form.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposeEndpoint {
    #[serde(default)]
    pub context_space: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub allowed_projects: Vec<String>,
    #[serde(default)]
    pub representations: Vec<String>,
    #[serde(default)]
    pub hidden_attributes: Vec<String>,
    #[serde(default)]
    pub entity_types: Vec<String>,
    #[serde(default)]
    pub rate_limits: Option<RateLimits>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimits {
    pub requests_per_minute: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst: Option<u32>,
}

/// The rendering: the manifests, the lane, and the form values the endpoint page takes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub lane: Lane,
    pub slug: String,
    pub endpoint: Value,
    pub policies: Vec<Value>,
    pub prefill: Value,
}

/// A fresh slug: 26 base32 characters, 130 bits, minted here and nowhere else (EP-02).
pub fn slug() -> String {
    let mut bytes = [0u8; 26];
    OsRng.fill_bytes(&mut bytes);
    // 256 is not a multiple of 32: masking to five bits keeps every character equally likely.
    bytes
        .iter()
        .map(|b| SLUG_ALPHABET[(b & 31) as usize] as char)
        .collect()
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':')
}

/// The manifests for `params` in `project`, or the first reason they cannot be rendered.
pub fn render(
    project: &str,
    org_domain: &str,
    params: &ProposeEndpoint,
) -> Result<Proposal, String> {
    let name = params.name.trim();
    let space = params.context_space.trim();
    if !is_dns1123(name) {
        return Err(format!("name '{name}' is not a DNS-1123 label"));
    }
    if !is_dns1123(space) {
        return Err(format!("contextSpace '{space}' is not a DNS-1123 label"));
    }
    let audience = params
        .audience
        .as_deref()
        .map(str::trim)
        .unwrap_or("project-list");
    if !AUDIENCES.contains(&audience) {
        return Err(format!(
            "audience '{audience}' is not one of {}",
            AUDIENCES.join(", ")
        ));
    }
    let projects: Vec<String> = params
        .allowed_projects
        .iter()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect();
    if let Some(bad) = projects.iter().find(|p| !is_dns1123(p)) {
        return Err(format!(
            "allowedProjects entry '{bad}' is not a DNS-1123 label"
        ));
    }
    if audience == "project-list" && projects.is_empty() {
        return Err(
            "audience project-list needs at least one project in allowedProjects".to_owned(),
        );
    }
    let representations: Vec<String> = if params.representations.is_empty() {
        vec!["ngsi-ld".to_owned(), "geojson".to_owned()]
    } else {
        params
            .representations
            .iter()
            .map(|r| r.trim().to_owned())
            .collect()
    };
    if let Some(bad) = representations
        .iter()
        .find(|r| !REPRESENTATIONS.contains(&r.as_str()))
    {
        return Err(format!(
            "representation '{bad}' is not one of {}",
            REPRESENTATIONS.join(", ")
        ));
    }
    if let Some(bad) = params.hidden_attributes.iter().find(|a| !is_identifier(a)) {
        return Err(format!(
            "hiddenAttributes entry '{bad}' is not an attribute name"
        ));
    }
    if let Some(bad) = params.entity_types.iter().find(|t| !is_identifier(t)) {
        return Err(format!("entityTypes entry '{bad}' is not a type name"));
    }
    if let Some(limits) = &params.rate_limits {
        if limits.requests_per_minute == 0 {
            return Err("rateLimits.requestsPerMinute must be greater than 0".to_owned());
        }
    }

    let slug = slug();
    let mut spec = json!({
        "contextSpaceRef": space,
        "slug": slug,
        "audience": audience,
        "enabledRepresentations": representations,
    });
    if audience == "project-list" {
        spec["allowedProjects"] = json!(projects);
    }
    if !params.hidden_attributes.is_empty() {
        spec["projection"] = json!({ "hiddenAttributes": params.hidden_attributes });
    }
    if let Some(limits) = &params.rate_limits {
        spec["rateLimits"] = serde_json::to_value(limits).unwrap_or(Value::Null);
    }
    let mut metadata = json!({
        "name": name,
        "namespace": project,
        "labels": { "joinedcontext.com/space": space },
    });
    if let Some(title) = params
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        metadata["title"] = json!({ "en": title });
    }
    let endpoint = json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": metadata,
        "spec": spec,
    });
    let lane = change::classify("Endpoint", Operation::Create, &endpoint["spec"]);

    // One draft Policy per assignee: the audience decides who, the request decides what.
    let assignees: Vec<(String, Value)> = match audience {
        "public" => vec![(
            "public".to_owned(),
            json!({ "kind": "role", "id": "public" }),
        )],
        "organization" => vec![(
            "organization".to_owned(),
            json!({ "kind": "group", "id": org_domain }),
        )],
        _ => projects
            .iter()
            .map(|p| (p.clone(), json!({ "kind": "group", "id": p })))
            .collect(),
    };
    let entities: Vec<Value> = params
        .entity_types
        .iter()
        .map(|t| json!({ "type": t }))
        .collect();
    let policies = assignees
        .into_iter()
        .map(|(suffix, assignee)| {
            json!({
                "apiVersion": API_VERSION,
                "kind": "Policy",
                "metadata": { "name": format!("{name}-{suffix}"), "namespace": project },
                "spec": {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": space },
                    "assigner": format!("did:web:{org_domain}"),
                    "assignee": assignee,
                    "operations": ["retrieveOps"],
                    "information": [{ "entities": entities }],
                },
            })
        })
        .collect();

    let listed: Vec<String> = if audience == "project-list" {
        projects.clone()
    } else {
        Vec::new()
    };
    let mut prefill = json!({
        "name": name,
        "contextSpaceRef": space,
        "slug": slug,
        "audience": audience,
        "allowedProjects": listed,
        "enabledRepresentations": representations,
        "hiddenAttributes": params.hidden_attributes,
    });
    if let Some(title) = endpoint["metadata"].get("title") {
        prefill["title"] = title.clone();
    }
    if let Some(limits) = &params.rate_limits {
        prefill["rateLimits"] = serde_json::to_value(limits).unwrap_or(Value::Null);
    }

    Ok(Proposal {
        lane,
        slug,
        endpoint,
        policies,
        prefill,
    })
}

pub(crate) static TOOL_FENCE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)```(?:json)?\s*(\{.*?\})\s*```").expect("a literal pattern")
});

/// The tool call in a model answer, when the answer is one: a fenced JSON object whose `tool`
/// is `propose_endpoint`. Anything else is an ordinary answer for the kit.
pub fn tool_call(answer: &str) -> Option<Result<ProposeEndpoint, String>> {
    for fence in TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) != Some("propose_endpoint") {
            continue;
        }
        return Some(
            serde_json::from_value::<ProposeEndpoint>(value)
                .map_err(|err| format!("the propose_endpoint call does not parse: {err}")),
        );
    }
    None
}

/// The prose of a tool answer: what stands before the fence, for the chat.
pub fn prose_of(answer: &str) -> String {
    TOOL_FENCE.replace_all(answer, "").trim().to_owned()
}

/// What `edit_endpoint` takes: the name of an endpoint that exists and only the fields that
/// change. Absent fields keep what the manifest says; unknown fields are ignored, as for a share.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditEndpoint {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub audience: Option<String>,
    #[serde(default)]
    pub allowed_projects: Option<Vec<String>>,
    #[serde(default)]
    pub add_representations: Vec<String>,
    #[serde(default)]
    pub remove_representations: Vec<String>,
    /// The whole list after the change; an empty list shows every attribute again.
    #[serde(default)]
    pub hidden_attributes: Option<Vec<String>>,
    #[serde(default)]
    pub requests_per_minute: Option<u32>,
}

/// One field the edit changes, as the chat and the tool step show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldChange {
    pub field: String,
    pub before: Value,
    pub after: Value,
}

/// An edited endpoint: the manifest with the change, the values the endpoint form opens with,
/// and what changed.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub endpoint: Value,
    pub prefill: Value,
    pub changes: Vec<FieldChange>,
}

/// Limits a request may set per minute; the rate classes of the form sit inside it.
const MAX_REQUESTS_PER_MINUTE: u32 = 100_000;

/// `params` applied to the endpoint of that name among `endpoints` (the project's manifests), or
/// the first reason it cannot be. Only the fields the call names change: the slug, the space,
/// the name and the labels stay what they were, so the Change is an edit of that endpoint.
pub fn edit(endpoints: &[Value], params: &EditEndpoint) -> Result<Edit, String> {
    let name = params.name.trim();
    let Some(found) = endpoints
        .iter()
        .find(|endpoint| endpoint["metadata"]["name"].as_str() == Some(name))
    else {
        let mut names: Vec<&str> = endpoints
            .iter()
            .filter_map(|endpoint| endpoint["metadata"]["name"].as_str())
            .collect();
        names.sort_unstable();
        return Err(if names.is_empty() {
            format!("there is no endpoint '{name}': this project has no endpoints")
        } else {
            format!(
                "there is no endpoint '{name}' in this project; its endpoints are {}",
                names.join(", ")
            )
        });
    };
    let mut endpoint = found.clone();
    if let Some(object) = endpoint.as_object_mut() {
        object.remove("status");
    }
    let mut changes = Vec::new();
    let mut record = |field: &str, before: Value, after: Value| {
        if before != after {
            changes.push(FieldChange {
                field: field.to_owned(),
                before,
                after,
            });
        }
    };

    if let Some(title) = params
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        let before = endpoint["metadata"]
            .get("title")
            .map(|t| json!(plain_title(t)))
            .unwrap_or(Value::Null);
        endpoint["metadata"]["title"] = json!(title);
        record("title", before, json!(title));
    }

    let spec = &mut endpoint["spec"];
    let current_audience = spec["audience"]
        .as_str()
        .unwrap_or("project-list")
        .to_owned();
    let audience = match params.audience.as_deref().map(str::trim) {
        Some(audience) if !AUDIENCES.contains(&audience) => {
            return Err(format!(
                "audience '{audience}' is not one of {}",
                AUDIENCES.join(", ")
            ));
        }
        Some(audience) => audience.to_owned(),
        None => current_audience.clone(),
    };
    let current_projects = spec
        .get("allowedProjects")
        .cloned()
        .unwrap_or_else(|| json!([]));
    if let Some(projects) = &params.allowed_projects {
        if audience != "project-list" {
            return Err(format!(
                "'{name}' is {audience}: allowedProjects only narrows a project-list audience; \
                 set audience to project-list with the projects"
            ));
        }
        if let Some(bad) = projects.iter().find(|p| !is_dns1123(p.trim())) {
            return Err(format!(
                "allowedProjects entry '{bad}' is not a DNS-1123 label"
            ));
        }
    }
    let projects: Vec<String> = match &params.allowed_projects {
        Some(projects) => projects.iter().map(|p| p.trim().to_owned()).collect(),
        None => serde_json::from_value(current_projects.clone()).unwrap_or_default(),
    };
    if audience == "project-list" {
        if projects.is_empty() {
            return Err(
                "audience project-list needs at least one project in allowedProjects".to_owned(),
            );
        }
        spec["allowedProjects"] = json!(projects);
    } else if let Some(object) = spec.as_object_mut() {
        object.remove("allowedProjects");
    }
    spec["audience"] = json!(audience);
    record("audience", json!(current_audience), json!(audience));
    record(
        "allowedProjects",
        current_projects,
        spec.get("allowedProjects")
            .cloned()
            .unwrap_or_else(|| json!([])),
    );

    let before: Vec<String> =
        serde_json::from_value(spec["enabledRepresentations"].clone()).unwrap_or_default();
    if let Some(bad) = params
        .add_representations
        .iter()
        .chain(&params.remove_representations)
        .find(|r| !REPRESENTATIONS.contains(&r.trim()))
    {
        return Err(format!(
            "representation '{bad}' is not one of {}",
            REPRESENTATIONS.join(", ")
        ));
    }
    let mut representations: Vec<String> = before
        .iter()
        .filter(|r| {
            !params
                .remove_representations
                .iter()
                .any(|gone| gone.trim() == r.as_str())
        })
        .cloned()
        .collect();
    for added in &params.add_representations {
        let added = added.trim();
        if !representations.iter().any(|r| r == added) {
            representations.push(added.to_owned());
        }
    }
    if representations.is_empty() {
        return Err(format!(
            "'{name}' would serve no representation; an endpoint serves at least one"
        ));
    }
    spec["enabledRepresentations"] = json!(representations);
    record(
        "enabledRepresentations",
        json!(before),
        json!(representations),
    );

    let hidden_before = spec["projection"]
        .get("hiddenAttributes")
        .cloned()
        .unwrap_or_else(|| json!([]));
    if let Some(hidden) = &params.hidden_attributes {
        if let Some(bad) = hidden.iter().find(|a| !is_identifier(a)) {
            return Err(format!(
                "hiddenAttributes entry '{bad}' is not an attribute name"
            ));
        }
        if hidden.is_empty() {
            if let Some(projection) = spec["projection"].as_object_mut() {
                projection.remove("hiddenAttributes");
                if projection.is_empty() {
                    if let Some(object) = spec.as_object_mut() {
                        object.remove("projection");
                    }
                }
            }
        } else {
            spec["projection"]["hiddenAttributes"] = json!(hidden);
        }
        record("hiddenAttributes", hidden_before, json!(hidden));
    }

    if let Some(per_minute) = params.requests_per_minute {
        if per_minute == 0 || per_minute > MAX_REQUESTS_PER_MINUTE {
            return Err(format!(
                "requestsPerMinute {per_minute} is outside 1..={MAX_REQUESTS_PER_MINUTE}"
            ));
        }
        let before = spec["rateLimits"]
            .get("requestsPerMinute")
            .cloned()
            .unwrap_or(Value::Null);
        spec["rateLimits"]["requestsPerMinute"] = json!(per_minute);
        record("requestsPerMinute", before, json!(per_minute));
    }

    if changes.is_empty() {
        return Err(format!("the request changes nothing on '{name}'"));
    }
    let prefill = form_values(&endpoint);
    Ok(Edit {
        endpoint,
        prefill,
        changes,
    })
}

/// A title as one string: the manifest's own string, or the English (else first) entry of the
/// legacy map.
fn plain_title(title: &Value) -> String {
    match title {
        Value::String(text) => text.clone(),
        Value::Object(map) => map
            .get("en")
            .or_else(|| map.values().next())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

/// The values the endpoint form opens with for an endpoint that exists (`existing: true`).
fn form_values(endpoint: &Value) -> Value {
    let spec = &endpoint["spec"];
    let mut prefill = json!({
        "existing": true,
        "name": endpoint["metadata"]["name"],
        "contextSpaceRef": spec["contextSpaceRef"],
        "slug": spec["slug"],
        "audience": spec["audience"],
        "enabledRepresentations": spec["enabledRepresentations"],
        "allowedProjects": spec.get("allowedProjects").cloned().unwrap_or_else(|| json!([])),
        "hiddenAttributes": spec["projection"].get("hiddenAttributes").cloned().unwrap_or_else(|| json!([])),
    });
    if let Some(title) = endpoint["metadata"].get("title") {
        prefill["title"] = json!(plain_title(title));
    }
    for field in ["rateLimits", "caching"] {
        if let Some(value) = spec.get(field) {
            prefill[field] = value.clone();
        }
    }
    prefill
}

/// The `edit_endpoint` call in a model answer, when the answer is one.
pub fn edit_call(answer: &str) -> Option<Result<EditEndpoint, String>> {
    for fence in TOOL_FENCE.captures_iter(answer) {
        let Ok(value) = serde_json::from_str::<Value>(&fence[1]) else {
            continue;
        };
        if value.get("tool").and_then(Value::as_str) != Some("edit_endpoint") {
            continue;
        }
        return Some(
            serde_json::from_value::<EditEndpoint>(value)
                .map_err(|err| format!("the edit_endpoint call does not parse: {err}")),
        );
    }
    None
}

/// What the model is shown of the project's endpoints, so it names one that exists.
pub fn endpoint_summaries(endpoints: &[Value]) -> Value {
    Value::Array(
        endpoints
            .iter()
            .map(|endpoint| {
                let spec = &endpoint["spec"];
                json!({
                    "name": endpoint["metadata"]["name"],
                    "title": endpoint["metadata"].get("title").map(plain_title),
                    "contextSpace": spec["contextSpaceRef"],
                    "audience": spec["audience"],
                    "representations": spec["enabledRepresentations"],
                    "allowedProjects": spec.get("allowedProjects"),
                    "hiddenAttributes": spec["projection"].get("hiddenAttributes"),
                    "requestsPerMinute": spec["rateLimits"].get("requestsPerMinute"),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        edit, edit_call, prose_of, render, slug, tool_call, EditEndpoint, FieldChange,
        ProposeEndpoint,
    };
    use crate::change::Lane;
    use serde_json::json;

    fn request() -> ProposeEndpoint {
        ProposeEndpoint {
            context_space: "helsinki".into(),
            name: "bikes-regional-transport".into(),
            allowed_projects: vec!["regional-transport".into()],
            hidden_attributes: vec!["maintenanceNote".into()],
            entity_types: vec!["BikeHireDockingStation".into()],
            ..Default::default()
        }
    }

    #[test]
    fn a_slug_is_26_base32_characters_and_fresh_every_time() {
        let a = slug();
        let b = slug();
        assert_eq!(a.len(), 26);
        assert!(a
            .bytes()
            .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c)));
        assert_ne!(a, b);
    }

    #[test]
    fn the_defaults_are_project_list_ngsi_ld_and_geojson_and_the_lane_is_yellow() {
        let proposal = render("helsinki", "hel.fi", &request()).expect("renders");
        assert_eq!(proposal.lane, Lane::Yellow);
        let spec = &proposal.endpoint["spec"];
        assert_eq!(spec["audience"], "project-list");
        assert_eq!(spec["allowedProjects"], json!(["regional-transport"]));
        assert_eq!(
            spec["enabledRepresentations"],
            json!(["ngsi-ld", "geojson"])
        );
        assert_eq!(
            spec["projection"]["hiddenAttributes"],
            json!(["maintenanceNote"])
        );
        assert_eq!(spec["slug"].as_str().map(str::len), Some(26));
        assert_eq!(proposal.endpoint["metadata"]["namespace"], "helsinki");
        assert_eq!(proposal.policies.len(), 1);
        let policy = &proposal.policies[0]["spec"];
        assert_eq!(
            policy["assignee"],
            json!({ "kind": "group", "id": "regional-transport" })
        );
        assert_eq!(policy["assigner"], "did:web:hel.fi");
        assert_eq!(policy["operations"], json!(["retrieveOps"]));
        assert_eq!(
            policy["information"][0]["entities"][0]["type"],
            "BikeHireDockingStation"
        );
        assert_eq!(
            proposal.prefill["hiddenAttributes"],
            json!(["maintenanceNote"])
        );
        assert_eq!(proposal.prefill["slug"], proposal.endpoint["spec"]["slug"]);
    }

    #[test]
    fn a_public_audience_is_the_red_lane_with_the_public_role() {
        let mut params = request();
        params.audience = Some("public".into());
        params.allowed_projects.clear();
        let proposal = render("helsinki", "hel.fi", &params).expect("renders");
        assert_eq!(proposal.lane, Lane::Red);
        assert!(proposal.endpoint["spec"].get("allowedProjects").is_none());
        assert_eq!(
            proposal.policies[0]["spec"]["assignee"],
            json!({ "kind": "role", "id": "public" })
        );
    }

    #[test]
    fn what_cannot_be_rendered_is_named() {
        let mut params = request();
        params.allowed_projects.clear();
        assert!(render("helsinki", "hel.fi", &params)
            .unwrap_err()
            .contains("allowedProjects"));
        let mut params = request();
        params.audience = Some("everyone".into());
        assert!(render("helsinki", "hel.fi", &params)
            .unwrap_err()
            .contains("audience"));
        let mut params = request();
        params.name = "Bikes!".into();
        assert!(render("helsinki", "hel.fi", &params)
            .unwrap_err()
            .contains("DNS-1123"));
        let mut params = request();
        params.representations = vec!["pdf".into()];
        assert!(render("helsinki", "hel.fi", &params)
            .unwrap_err()
            .contains("representation"));
        let mut params = request();
        params.hidden_attributes = vec!["<script>".into()];
        assert!(render("helsinki", "hel.fi", &params)
            .unwrap_err()
            .contains("hiddenAttributes"));
    }

    fn endpoints() -> Vec<serde_json::Value> {
        vec![
            json!({
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": "Endpoint",
                "metadata": {
                    "name": "helsinki-kpi",
                    "namespace": "helsinki",
                    "labels": { "joinedcontext.com/space": "helsinki-kpi" },
                    "title": { "en": "Helsinki indicators" }
                },
                "spec": {
                    "contextSpaceRef": "helsinki-kpi",
                    "slug": "abcdefghijklmnopqrstuvwxyz",
                    "audience": "project-list",
                    "allowedProjects": ["helsinki-mobility"],
                    "enabledRepresentations": ["ngsi-ld", "csv"],
                    "rateLimits": { "requestsPerMinute": 600, "burst": 50 }
                },
                "status": { "phase": "Live" }
            }),
            json!({
                "metadata": { "name": "helsinki-weather", "namespace": "helsinki" },
                "spec": { "contextSpaceRef": "helsinki", "slug": "weatherweatherweatherweath", "audience": "public", "enabledRepresentations": ["ngsi-ld"] }
            }),
        ]
    }

    fn edit_of(value: serde_json::Value) -> EditEndpoint {
        serde_json::from_value(value).expect("an edit call")
    }

    #[test]
    fn an_edit_adds_and_removes_representations_and_keeps_the_slug_space_and_labels() {
        let edited = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-kpi", "addRepresentations": ["geojson", "csv"], "removeRepresentations": ["ngsi-ld"] })),
        )
        .expect("edits");
        let spec = &edited.endpoint["spec"];
        assert_eq!(spec["enabledRepresentations"], json!(["csv", "geojson"]));
        assert_eq!(spec["slug"], "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(spec["contextSpaceRef"], "helsinki-kpi");
        assert_eq!(spec["rateLimits"]["burst"], 50);
        assert_eq!(
            edited.endpoint["metadata"]["labels"]["joinedcontext.com/space"],
            "helsinki-kpi"
        );
        assert!(
            edited.endpoint.get("status").is_none(),
            "status is never written"
        );
        assert_eq!(
            edited.changes,
            vec![FieldChange {
                field: "enabledRepresentations".into(),
                before: json!(["ngsi-ld", "csv"]),
                after: json!(["csv", "geojson"]),
            }]
        );
        assert_eq!(edited.prefill["existing"], true);
        assert_eq!(edited.prefill["slug"], "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(edited.prefill["title"], "Helsinki indicators");
        assert_eq!(
            edited.prefill["allowedProjects"],
            json!(["helsinki-mobility"])
        );
    }

    #[test]
    fn going_public_drops_the_project_list_and_hiding_sets_the_projection() {
        let edited = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-kpi", "audience": "public", "hiddenAttributes": ["address"], "requestsPerMinute": 1200 })),
        )
        .expect("edits");
        let spec = &edited.endpoint["spec"];
        assert_eq!(spec["audience"], "public");
        assert!(spec.get("allowedProjects").is_none());
        assert_eq!(spec["projection"]["hiddenAttributes"], json!(["address"]));
        assert_eq!(
            spec["rateLimits"],
            json!({ "requestsPerMinute": 1200, "burst": 50 })
        );
        let fields: Vec<&str> = edited.changes.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "audience",
                "allowedProjects",
                "hiddenAttributes",
                "requestsPerMinute"
            ]
        );
        assert_eq!(edited.prefill["allowedProjects"], json!([]));
        assert_eq!(edited.prefill["hiddenAttributes"], json!(["address"]));
    }

    #[test]
    fn what_an_edit_cannot_do_is_named() {
        let unknown = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-parking", "audience": "public" })),
        )
        .unwrap_err();
        assert!(
            unknown.contains("helsinki-kpi, helsinki-weather"),
            "{unknown}"
        );
        assert!(edit(&[], &edit_of(json!({ "name": "x" })))
            .unwrap_err()
            .contains("no endpoints"));
        let bad_audience = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-kpi", "audience": "everyone" })),
        )
        .unwrap_err();
        assert!(bad_audience.contains("audience"), "{bad_audience}");
        let bad_representation = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-kpi", "addRepresentations": ["pdf"] })),
        )
        .unwrap_err();
        assert!(
            bad_representation.contains("representation 'pdf'"),
            "{bad_representation}"
        );
        let none_left = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-weather", "removeRepresentations": ["ngsi-ld"] })),
        )
        .unwrap_err();
        assert!(none_left.contains("at least one"), "{none_left}");
        let narrowing_public = edit(
            &endpoints(),
            &edit_of(
                json!({ "name": "helsinki-weather", "allowedProjects": ["helsinki-mobility"] }),
            ),
        )
        .unwrap_err();
        assert!(
            narrowing_public.contains("project-list"),
            "{narrowing_public}"
        );
        let empty_list = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-weather", "audience": "project-list" })),
        )
        .unwrap_err();
        assert!(empty_list.contains("allowedProjects"), "{empty_list}");
        let nothing = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-weather", "audience": "public" })),
        )
        .unwrap_err();
        assert!(nothing.contains("changes nothing"), "{nothing}");
        let limit = edit(
            &endpoints(),
            &edit_of(json!({ "name": "helsinki-weather", "requestsPerMinute": 0 })),
        )
        .unwrap_err();
        assert!(limit.contains("requestsPerMinute"), "{limit}");
    }

    #[test]
    fn an_edit_call_is_told_from_a_share_call() {
        let answer = "Adding CSV.\n\n```json\n{\"tool\": \"edit_endpoint\", \"name\": \"helsinki-news\", \"addRepresentations\": [\"csv\"]}\n```\n";
        let call = edit_call(answer).expect("an edit call").expect("parses");
        assert_eq!(call.name, "helsinki-news");
        assert_eq!(call.add_representations, vec!["csv"]);
        assert!(tool_call(answer).is_none());
        assert!(edit_call("```json\n{\"tool\": \"propose_endpoint\"}\n```").is_none());
    }

    #[test]
    fn a_tool_answer_is_told_from_a_kit_answer() {
        let answer = "I will share the stations.\n\n```json\n{\"tool\": \"propose_endpoint\", \"contextSpace\": \"helsinki\", \"name\": \"bikes\", \"allowedProjects\": [\"transport\"], \"extra\": 1}\n```\n";
        let call = tool_call(answer).expect("a tool call").expect("parses");
        assert_eq!(call.name, "bikes");
        assert_eq!(call.allowed_projects, vec!["transport"]);
        assert_eq!(prose_of(answer), "I will share the stations.");
        assert!(
            tool_call("```text\nspec.json\n<<<<<<< SEARCH\n=======\n{}\n>>>>>>> REPLACE\n```")
                .is_none()
        );
        assert!(tool_call("```json\n{\"title\": \"x\"}\n```").is_none());
    }
}
