//! `change_resource` (AG-77, AG-73): the conversation changes or removes a resource of any kind
//! by its name. The platform reads the manifest, applies the model's JSON merge patch (RFC 7386)
//! and checks the result; the kind's page opens with the change filled in, or with its removal
//! dialog, and the person proposes from there. Nothing is proposed here.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::share::TOOL_FENCE;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeResource {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    /// The fields that change, as a JSON merge patch of the manifest; `null` removes a field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<Value>,
    /// A data model's change: the model editor's operations on its LinkML source (DM-13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Vec<crate::agents::model_change::Operation>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delete: bool,
    /// A new Dashboard: `patch` holds its spec and `layers` the new Layers its pages draw.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub create: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<NewLayer>,
}

/// A Layer a new dashboard draws, created with it (UI-18).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct NewLayer {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub spec: Value,
}

/// A manifest of this project's `kind`, as a new resource is written.
pub fn manifest(kind: &str, project: &str, name: &str, spec: Value) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": kind,
        "metadata": { "name": name, "namespace": project },
        "spec": spec,
    })
}

/// The kinds one sentence creates from the chat (AG-45, UI-45): the kind's page opens on the
/// checked draft, the form is already filled and the person proposes it. A Dashboard is created
/// with its layers by its own step; every other kind is changed from the chat, never created,
/// because its create needs a page of its own (a data model is drawn, a space is completed from
/// files, an endpoint is shared, a role is granted).
pub const CREATABLE: [&str; 3] = ["ContextSpace", "DataSource", "Pipeline"];

/// The manifest of a new resource, from the fields the model sent as its `patch`: its `spec`, and
/// `metadata.title` when the sentence named one. The kind, the name and the namespace are the
/// platform's; a patch that carries no spec is refused in words the model can act on.
pub fn new_manifest(
    kind: &str,
    namespace: &str,
    name: &str,
    patch: Option<&Value>,
) -> Result<Value, String> {
    if !CREATABLE.contains(&kind) {
        return Err(format!(
            "a new {kind} is not created from the chat; the kinds created here are {}",
            CREATABLE.join(", ")
        ));
    }
    let patch = patch.ok_or_else(|| {
        format!(
            "a new {kind} carries its fields as the patch: {{\"spec\": {{…}}}}, and its title as \
             {{\"metadata\": {{\"title\": {{\"en\": \"…\"}}}}}}"
        )
    })?;
    let manifest = patched(&manifest(kind, namespace, name, json!({})), patch)?;
    if !manifest["spec"]
        .as_object()
        .is_some_and(|spec| !spec.is_empty())
    {
        return Err(format!(
            "a new {kind} needs a spec; the patch carried none: {patch}"
        ));
    }
    Ok(manifest)
}

/// The spec a new resource's `patch` carries: `{spec: {…}}` as a merge patch of an empty
/// manifest, or the spec itself.
pub fn spec_of(patch: &Value) -> Option<Value> {
    let spec = patch.get("spec").unwrap_or(patch);
    spec.is_object().then(|| spec.clone())
}

/// The attributes a layer's encodings and popup name, in order, each once.
pub fn named_attributes(layer: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let encoded = ["/colorBy/property", "/sizeBy/property"]
        .into_iter()
        .filter_map(|pointer| layer.pointer(pointer).and_then(Value::as_str));
    let popup = layer
        .get("popupProperties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    for name in encoded.chain(popup) {
        if !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// The call of the tool named `tool` in a model answer, when the answer holds one.
pub fn call_of<T: DeserializeOwned>(answer: &str, tool: &str) -> Option<Result<T, String>> {
    TOOL_FENCE.captures_iter(answer).find_map(|fence| {
        let value = serde_json::from_str::<Value>(&fence[1]).ok()?;
        (value.get("tool").and_then(Value::as_str) == Some(tool)).then(|| {
            serde_json::from_value::<T>(value)
                .map_err(|err| format!("the {tool} call does not parse: {err}"))
        })
    })
}

/// The `change_resource` call in a model answer, when the answer is one.
pub fn tool_call(answer: &str) -> Option<Result<ChangeResource, String>> {
    call_of(answer, "change_resource")
}

/// RFC 7386: an object patch merges key by key, `null` removes a key, anything else replaces.
fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(changes) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    if let Value::Object(fields) = target {
        for (key, value) in changes {
            if value.is_null() {
                fields.remove(key);
            } else {
                merge(fields.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
    }
}

/// The manifest with the patch applied and without status (MF-04). The kind, the name and the
/// namespace stay: another one would be another resource.
pub fn patched(current: &Value, patch: &Value) -> Result<Value, String> {
    if !patch.is_object() {
        return Err(
            "the patch is a JSON merge patch: an object holding only the fields that change"
                .to_owned(),
        );
    }
    let mut before = current.clone();
    if let Value::Object(fields) = &mut before {
        fields.remove("status");
    }
    let mut after = before.clone();
    merge(&mut after, patch);
    if let Value::Object(fields) = &mut after {
        fields.remove("status");
    }
    for pointer in [
        "/apiVersion",
        "/kind",
        "/metadata/name",
        "/metadata/namespace",
    ] {
        if after.pointer(pointer) != before.pointer(pointer) {
            return Err(format!(
                "{pointer} stays as it is; a different one would be another resource"
            ));
        }
    }
    if after == before {
        return Err("the patch changes nothing in the manifest".to_owned());
    }
    Ok(after)
}

/// What a pipeline's test exercises: what it reads, how it maps and where it writes.
const TESTED: [&str; 5] = ["source", "compute", "output", "targetEndpoint", "class"];

/// Whether a pipeline change reaches what its test exercises; a pause, a period, a schedule or a
/// quota does not (PL-45).
pub fn reaches_the_test(before: &Value, after: &Value) -> bool {
    TESTED
        .iter()
        .any(|field| before["spec"].get(field) != after["spec"].get(field))
}

/// The Portal page a kind is changed on: its own section where it has one, else its list.
fn page<'a>(kind: &str, plural: &'a str) -> &'a str {
    match kind {
        "ContextSpace" => "spaces",
        "DataModel" => "models",
        "Dashboard" | "Layer" => "dashboards",
        "ServiceAccount" | "Role" | "RoleBinding" => "access",
        "CkanInstance" => "ckan",
        _ => plural,
    }
}

/// Where the person reviews the change: the kind's page with the resource's editor open
/// (`?edit=`), or its removal dialog (`?delete=`). The endpoint form opens from the draft it is
/// handed, as it did for `edit_endpoint`.
/// Where the person reviews a new resource: the kind's page, which opens on the draft the
/// conversation kept for them (`?draft=`, appended by the dock from the event's draft) and fills
/// the form from it (AG-45, UI-45).
pub fn create_route(project: &str, kind: &str, plural: &str) -> String {
    format!("/projects/{project}/{}", page(kind, plural))
}

pub fn route(project: &str, kind: &str, plural: &str, name: &str, delete: bool) -> String {
    let page = page(kind, plural);
    match (kind, delete) {
        ("Endpoint", false) => format!("/projects/{project}/{page}"),
        (_, true) => format!("/projects/{project}/{page}?delete={name}"),
        (_, false) => format!("/projects/{project}/{page}?edit={name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipeline() -> Value {
        json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Pipeline",
            "metadata": { "name": "hel-news", "namespace": "helsinki" },
            "spec": { "enabled": true, "schedule": { "every": "15m" }, "compute": { "kind": "bloblang" } },
            "status": { "phase": "Live" }
        })
    }

    #[test]
    fn a_merge_patch_changes_only_the_fields_it_names_and_null_removes_one() {
        let after = patched(
            &pipeline(),
            &json!({ "spec": { "enabled": false, "compute": null } }),
        )
        .expect("patched");
        assert_eq!(
            after["spec"],
            json!({ "enabled": false, "schedule": { "every": "15m" } })
        );
        assert!(after.get("status").is_none(), "status is never written");
    }

    #[test]
    fn a_schedule_a_mapping_and_a_source_address_are_patched_in_place() {
        let mapped = patched(
            &pipeline(),
            &json!({ "spec": { "period": "5m", "compute": { "bloblang": "root.availableBikeNumber = this.num_bikes_available" } } }),
        )
        .expect("patched");
        assert_eq!(mapped["spec"]["period"], "5m");
        // A merge patch keeps the compute's kind beside the new mapping.
        assert_eq!(
            mapped["spec"]["compute"],
            json!({ "kind": "bloblang", "bloblang": "root.availableBikeNumber = this.num_bikes_available" })
        );

        let source = json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "DataSource",
            "metadata": { "name": "hsl-bikes", "namespace": "helsinki" },
            "spec": { "type": "http", "http": { "url": "https://feeds.example/a.json", "timeout": "10s" } }
        });
        let moved = patched(
            &source,
            &json!({ "spec": { "http": { "url": "https://feeds.example/b.json" } } }),
        )
        .expect("patched");
        assert_eq!(
            moved["spec"]["http"],
            json!({ "url": "https://feeds.example/b.json", "timeout": "10s" })
        );
    }

    #[test]
    fn a_pause_or_a_schedule_does_not_reach_the_test_and_a_mapping_does() {
        let paused =
            patched(&pipeline(), &json!({ "spec": { "enabled": false } })).expect("paused");
        assert!(!reaches_the_test(&pipeline(), &paused));
        let every = patched(
            &pipeline(),
            &json!({ "spec": { "schedule": { "every": "5m" } } }),
        )
        .expect("rescheduled");
        assert!(!reaches_the_test(&pipeline(), &every));
        let mapped = patched(
            &pipeline(),
            &json!({ "spec": { "compute": { "bloblang": "root = this" } } }),
        )
        .expect("mapped");
        assert!(reaches_the_test(&pipeline(), &mapped));
    }

    #[test]
    fn a_patch_that_renames_changes_nothing_or_is_no_object_is_refused() {
        let renamed = patched(&pipeline(), &json!({ "metadata": { "name": "other" } }));
        assert!(renamed.is_err_and(|e| e.contains("/metadata/name")));
        let same = patched(&pipeline(), &json!({ "spec": { "enabled": true } }));
        assert!(same.is_err_and(|e| e.contains("changes nothing")));
        let listed = patched(&pipeline(), &json!(["suspended"]));
        assert!(listed.is_err_and(|e| e.contains("merge patch")));
    }

    #[test]
    fn a_new_dashboard_reads_its_spec_either_way_and_its_layers_name_their_attributes_once() {
        let spec = json!({ "title": "Bikes", "pages": [{ "layers": ["stations"] }] });
        assert_eq!(spec_of(&json!({ "spec": spec })), Some(spec.clone()));
        assert_eq!(spec_of(&spec), Some(spec.clone()));
        assert_eq!(spec_of(&json!(["pages"])), None);
        let call = tool_call("```json\n{\"tool\":\"change_resource\",\"kind\":\"Dashboard\",\"name\":\"bikes\",\"create\":true,\"patch\":{\"spec\":{}},\"layers\":[{\"name\":\"stations\",\"spec\":{\"entityType\":\"BikeHireDockingStation\"}}]}\n```")
            .expect("a call")
            .expect("parses");
        assert!(call.create);
        assert_eq!(call.layers[0].name, "stations");
        let layer = json!({
            "colorBy": { "property": "availableBikeNumber" },
            "sizeBy": { "property": "capacity" },
            "popupProperties": ["name", "availableBikeNumber"]
        });
        assert_eq!(
            named_attributes(&layer),
            ["availableBikeNumber", "capacity", "name"]
        );
        assert_eq!(
            manifest("Layer", "helsinki", "stations", json!({}))["metadata"],
            json!({ "name": "stations", "namespace": "helsinki" })
        );
    }

    #[test]
    fn the_call_is_read_from_the_fence_with_a_patch_or_a_removal() {
        let answer = "Pausing it.\n\n```json\n{\"tool\":\"change_resource\",\"kind\":\"Pipeline\",\"name\":\"hel-news\",\"patch\":{\"spec\":{\"enabled\":false}}}\n```\n";
        let call = tool_call(answer).expect("a call").expect("parses");
        assert_eq!(
            (call.kind.as_str(), call.name.as_str()),
            ("Pipeline", "hel-news")
        );
        assert!(!call.delete);
        let removal = "```json\n{\"tool\":\"change_resource\",\"kind\":\"ContextSpace\",\"name\":\"test-space\",\"delete\":true}\n```";
        assert!(tool_call(removal).expect("a call").expect("parses").delete);
        assert!(tool_call("```json\n{\"tool\":\"navigate\",\"route\":\"/\"}\n```").is_none());
    }

    #[test]
    fn each_kind_opens_on_its_own_page() {
        assert_eq!(
            route("helsinki", "Pipeline", "pipelines", "hel-news", false),
            "/projects/helsinki/pipelines?edit=hel-news"
        );
        assert_eq!(
            route("helsinki", "ContextSpace", "spaces", "test-space", true),
            "/projects/helsinki/spaces?delete=test-space"
        );
        assert_eq!(
            route("helsinki", "Endpoint", "endpoints", "helsinki-bikes", false),
            "/projects/helsinki/endpoints"
        );
        assert_eq!(
            route("helsinki", "DataModel", "datamodels", "air", false),
            "/projects/helsinki/models?edit=air"
        );
        assert_eq!(
            route(
                "helsinki",
                "ContextSourceRegistration",
                "csrs",
                "zvolen",
                true
            ),
            "/projects/helsinki/csrs?delete=zvolen"
        );
    }

    /// AG-45: one sentence carries the name and the fields into the form, so the person types
    /// nothing again. A new resource's page opens on the draft, not on a resource that exists.
    #[test]
    fn a_new_resource_carries_the_name_the_spec_and_the_title_the_sentence_gave() {
        let space = new_manifest(
            "ContextSpace",
            "helsinki",
            "ovzdusie",
            Some(&json!({
                "metadata": { "title": { "en": "Air quality" } },
                "spec": { "defaultLocale": "en", "isSandbox": false }
            })),
        )
        .expect("a new space");
        assert_eq!(space["metadata"]["name"], "ovzdusie");
        assert_eq!(space["metadata"]["namespace"], "helsinki");
        assert_eq!(space["metadata"]["title"]["en"], "Air quality");
        assert_eq!(space["spec"]["defaultLocale"], "en");
        assert_eq!(space["apiVersion"], "joinedcontext.com/v1alpha1");

        let source = new_manifest(
            "DataSource",
            "helsinki",
            "aq-opendata",
            Some(&json!({ "spec": { "type": "http", "http": { "url": "https://opendata.example.org/aq.json" } } })),
        )
        .expect("a new data source");
        assert_eq!(
            source["spec"]["http"]["url"],
            "https://opendata.example.org/aq.json"
        );
        assert_eq!(source["metadata"]["title"], Value::Null);
    }

    #[test]
    fn a_new_resource_without_a_spec_or_of_a_kind_with_its_own_page_is_refused_with_the_reason() {
        let no_spec = new_manifest(
            "Pipeline",
            "helsinki",
            "bikes",
            Some(&json!({ "metadata": { "title": { "en": "Bikes" } } })),
        )
        .expect_err("a pipeline without a spec");
        assert!(no_spec.contains("needs a spec"), "{no_spec}");

        let nothing = new_manifest("DataSource", "helsinki", "aq", None).expect_err("no patch");
        assert!(nothing.contains("\"spec\""), "{nothing}");

        // A name the patch tries to change would be another resource; `patched` holds that line.
        let renamed = new_manifest(
            "ContextSpace",
            "helsinki",
            "ovzdusie",
            Some(&json!({ "metadata": { "name": "other" }, "spec": { "defaultLocale": "en" } })),
        )
        .expect_err("a renamed space");
        assert!(renamed.contains("metadata/name"), "{renamed}");

        for kind in ["DataModel", "Endpoint", "RoleBinding", "Dashboard", "Layer"] {
            let refused = new_manifest(kind, "helsinki", "x", Some(&json!({ "spec": { "a": 1 } })))
                .expect_err("not created from the chat");
            assert!(
                refused.contains("ContextSpace, DataSource, Pipeline"),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_new_resource_opens_the_kinds_page_and_the_dock_appends_its_draft() {
        assert_eq!(
            create_route("helsinki", "ContextSpace", "spaces"),
            "/projects/helsinki/spaces"
        );
        assert_eq!(
            create_route("helsinki", "DataSource", "datasources"),
            "/projects/helsinki/datasources"
        );
        assert_eq!(
            create_route("helsinki", "Pipeline", "pipelines"),
            "/projects/helsinki/pipelines"
        );
    }
}
