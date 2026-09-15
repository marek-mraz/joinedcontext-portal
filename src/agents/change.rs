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
}
