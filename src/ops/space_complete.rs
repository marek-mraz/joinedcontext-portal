//! `jc_space_complete`: completes a partially defined ContextSpace from files or an endpoint (T-0642).

use std::collections::BTreeMap;

use jcctl::pipeline_test::{Sample, SampleFormat};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::api::import::ImportReport;
use crate::change::{Change, Lane};
use crate::error::ApiError;
use crate::ops::verdict::{Finding, Level, Verdict};
use crate::ops::{self, draft_store, Caller, OpError};
use crate::resource::{self, is_dns1123, API_VERSION};
use crate::state::AppState;
use crate::tools::model_tools;

const MAX_TOTAL_FILES_BYTES: usize = 10 * 1024 * 1024; // 10 MiB (DM-55)
/// The longest description a completion carries onto its drafts (AG-73).
const MAX_DESCRIPTION_CHARS: usize = 4000;

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputFile {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceCompleteInput {
    #[serde(default)]
    pub space: Option<String>,
    #[serde(default)]
    pub files: Option<Vec<InputFile>>,
    #[serde(default)]
    pub url: Option<String>,
    /// The entity type the inferred model and the pipeline name, PascalCase (AG-73).
    #[serde(default)]
    pub type_name: Option<String>,
    /// What the data is, carried onto the drafted model, space and source (AG-73).
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub propose: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompletedDraft {
    pub kind: String,
    pub name: String,
    pub inferred: bool,
    pub manifest: Value,
    pub verdict: Option<Verdict>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCompleteOutput {
    pub space: String,
    pub found: Vec<String>,
    pub drafts: Vec<CompletedDraft>,
    pub propose_ready: bool,
    pub lane: Lane,
    pub change: Option<Change>,
}

pub fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "space": { "type": "string", "description": "Context space name (dns-1123 label)" },
            "files": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["name", "content"],
                    "additionalProperties": false
                },
                "description": "Text files defining or describing the space"
            },
            "url": { "type": "string", "description": "HTTP endpoint (JSON, GeoJSON, GBFS or NGSI-LD)" },
            "typeName": {
                "type": "string",
                "pattern": "^[A-Z][A-Za-z0-9]{0,62}$",
                "description": "Entity type of the inferred model and the pipeline's output, PascalCase"
            },
            "description": {
                "type": "string",
                "maxLength": MAX_DESCRIPTION_CHARS,
                "description": "What the data is, from the person's description and specification"
            },
            "propose": { "type": "boolean", "description": "Propose all drafts as a single change if ready" }
        },
        "additionalProperties": false
    })
}

pub fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "space": { "type": "string" },
            "found": { "type": "array", "items": { "type": "string" } },
            "drafts": { "type": "array", "items": { "type": "object" } },
            "proposeReady": { "type": "boolean" },
            "lane": { "type": "string" },
            "change": { "type": ["object", "null"] }
        },
        "required": ["space", "found", "drafts", "proposeReady", "lane"]
    })
}

pub fn validate_input(val: &Value) -> Result<(), OpError> {
    let input: SpaceCompleteInput = serde_json::from_value(val.clone()).map_err(|e| {
        let (path, message) = ops::serde_error_path_and_message(&e);
        OpError::InvalidInput { path, message }
    })?;

    let has_files = input.files.as_ref().is_some_and(|f| !f.is_empty());
    let has_url = input.url.as_ref().is_some_and(|u| !u.trim().is_empty());

    if has_files == has_url {
        return Err(OpError::InvalidInput {
            path: "/".into(),
            message: "exactly one of files or url is required".into(),
        });
    }

    if let Some(type_name) = &input.type_name {
        if !is_type_name(type_name) {
            return Err(OpError::InvalidInput {
                path: "/typeName".into(),
                message: format!(
                    "'{type_name}' is not a PascalCase type name (a capital letter, then up to 62 letters or digits)"
                ),
            });
        }
    }
    if input
        .description
        .as_ref()
        .is_some_and(|d| d.chars().count() > MAX_DESCRIPTION_CHARS)
    {
        return Err(OpError::InvalidInput {
            path: "/description".into(),
            message: format!("description is longer than {MAX_DESCRIPTION_CHARS} characters"),
        });
    }

    if let Some(files) = &input.files {
        let total_bytes: usize = files.iter().map(|f| f.content.len()).sum();
        if total_bytes > MAX_TOTAL_FILES_BYTES {
            return Err(OpError::InvalidInput {
                path: "/files".into(),
                message: format!("total file size exceeds limit of {MAX_TOTAL_FILES_BYTES} bytes"),
            });
        }
    }

    Ok(())
}

fn sanitize_dns1123(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if (c == '-' || c == '_' || c == ' ' || c == '.')
            && !out.ends_with('-')
            && !out.is_empty()
        {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "space".into()
    } else {
        trimmed.chars().take(63).collect()
    }
}

/// A NGSI-LD type name the model and the pipeline can carry: PascalCase letters and digits.
fn is_type_name(raw: &str) -> bool {
    let mut chars = raw.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && raw.len() <= 63
        && chars.all(|c| c.is_ascii_alphanumeric())
}

/// The class a completion infers: the type the person named, else the sample file's name,
/// else the space's own name made singular. Never `Entity`, the model's abstract base class.
fn class_name_of(type_name: Option<&str>, sample_name: Option<&str>, space_name: &str) -> String {
    if let Some(name) = type_name.filter(|name| is_type_name(name)) {
        return name.to_owned();
    }
    let derived = to_pascal_case(sample_name.unwrap_or(space_name));
    let derived = if derived == "Entity" {
        to_pascal_case(space_name)
    } else {
        derived
    };
    if derived.starts_with(|c: char| c.is_ascii_uppercase()) && derived != "Entity" {
        derived
    } else {
        format!("Feed{derived}")
    }
}

/// The inferred LinkML source with `description` on `class`, when the source parses and the
/// class carries none yet; the source unchanged otherwise.
fn describe_class(source: &str, class: &str, description: &str) -> String {
    let Ok(mut doc) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(source) else {
        return source.to_owned();
    };
    let Some(entry) = doc
        .get_mut("classes")
        .and_then(|classes| classes.get_mut(class))
    else {
        return source.to_owned();
    };
    if entry.is_null() {
        *entry = serde_yaml_ng::Value::Mapping(serde_yaml_ng::Mapping::new());
    }
    let Some(mapping) = entry.as_mapping_mut() else {
        return source.to_owned();
    };
    if mapping.contains_key("description") {
        return source.to_owned();
    }
    mapping.insert("description".into(), description.into());
    serde_yaml_ng::to_string(&doc).unwrap_or_else(|_| source.to_owned())
}

/// `metadata.description` of a drafted manifest, in the same language map as its title, unless
/// the manifest already says something (AG-73).
fn describe(manifest: &mut Value, description: Option<&str>) {
    let Some(description) = description.map(str::trim).filter(|d| !d.is_empty()) else {
        return;
    };
    if let Some(metadata) = manifest.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata
            .entry("description")
            .or_insert_with(|| json!({ "en": description }));
    }
}

fn to_pascal_case(raw: &str) -> String {
    let stem = raw
        .strip_suffix(".csv")
        .or_else(|| raw.strip_suffix(".json"))
        .or_else(|| raw.strip_suffix(".geojson"))
        .unwrap_or(raw);
    let mut s = stem.to_string();
    if s.ends_with('s') && !s.ends_with("ss") && s.len() > 1 {
        s.pop();
    }
    let mut result = String::new();
    let mut cap_next = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if cap_next {
                result.extend(c.to_uppercase());
                cap_next = false;
            } else {
                result.push(c);
            }
        } else {
            cap_next = true;
        }
    }
    if result.is_empty() {
        "Entity".into()
    } else {
        result
    }
}

fn words_capitalized(raw: &str) -> String {
    let parts: Vec<&str> = raw
        .split(['-', '_', ' '])
        .filter(|p| !p.is_empty())
        .collect();
    parts
        .iter()
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn run(
    caller: &Caller,
    state: &AppState,
    project: &str,
    val: Value,
) -> Result<Value, OpError> {
    let input: SpaceCompleteInput = serde_json::from_value(val).map_err(|e| {
        let (path, message) = ops::serde_error_path_and_message(&e);
        OpError::InvalidInput { path, message }
    })?;

    // Determine space name
    let space_name = if let Some(s) = input.space.as_deref().filter(|s| !s.trim().is_empty()) {
        sanitize_dns1123(s)
    } else if let Some(url_str) = input.url.as_deref().filter(|u| !u.trim().is_empty()) {
        let seg = url_str
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .and_then(|s| s.split('?').next())
            .unwrap_or("space");
        let stem = seg
            .strip_suffix(".json")
            .or_else(|| seg.strip_suffix(".geojson"))
            .unwrap_or(seg);
        sanitize_dns1123(stem)
    } else if let Some(files) = &input.files {
        let first_name = files.first().map(|f| f.name.as_str()).unwrap_or("space");
        let seg = if let Some((dir, _)) = first_name.split_once('/') {
            dir
        } else {
            first_name
                .strip_suffix(".csv")
                .or_else(|| first_name.strip_suffix(".json"))
                .or_else(|| first_name.strip_suffix(".geojson"))
                .unwrap_or(first_name)
        };
        sanitize_dns1123(seg)
    } else {
        "space".into()
    };

    if !is_dns1123(&space_name) {
        return Err(OpError::InvalidInput {
            path: "/space".into(),
            message: format!("'{space_name}' is not a valid DNS-1123 label"),
        });
    }

    let mut found_kinds = Vec::new();
    let mut manifests: BTreeMap<String, Value> = BTreeMap::new();
    let mut linkml_source: Option<String> = None;
    let mut bloblang_content: Option<String> = None;
    let mut sample_bytes: Option<(String, Vec<u8>)> = None;
    let mut described_source_spec: Option<(String, Value, Vec<Finding>)> = None;

    if let Some(url_str) = &input.url {
        let ds_manifest = json!({
            "apiVersion": API_VERSION,
            "kind": "DataSource",
            "metadata": {
                "name": format!("{space_name}-source"),
                "namespace": project,
                "title": { "en": format!("{} source", words_capitalized(&space_name)) }
            },
            "spec": {
                "type": "http",
                "http": {
                    "url": url_str.trim(),
                    "verb": "GET",
                    "headers": { "Accept": "application/json" },
                    "timeout": "15s"
                }
            }
        });
        let mut ds_manifest = ds_manifest;
        describe(&mut ds_manifest, input.description.as_deref());
        manifests.insert("DataSource".into(), ds_manifest);
    }

    if let Some(files) = &input.files {
        for file in files {
            let name = file.name.as_str();
            if name.ends_with(".yaml") || name.ends_with(".yml") {
                if name.ends_with(".linkml.yaml") {
                    linkml_source = Some(file.content.clone());
                    continue;
                }
                for doc in serde_yaml_ng::Deserializer::from_str(&file.content) {
                    if let Ok(val) = Value::deserialize(doc) {
                        if let Some(kind) = val.get("kind").and_then(Value::as_str) {
                            found_kinds.push(kind.to_string());
                            manifests.insert(kind.to_string(), val);
                        }
                    }
                }
            } else if name.ends_with(".blobl") {
                bloblang_content = Some(file.content.clone());
            } else if (name.ends_with(".csv")
                || name.ends_with(".json")
                || name.ends_with(".geojson"))
                && sample_bytes.is_none()
            {
                sample_bytes = Some((name.to_string(), file.content.as_bytes().to_vec()));
            } else if name.eq_ignore_ascii_case("readme.md") || name.eq_ignore_ascii_case("readme")
            {
                // Find candidate URL in README
                for word in file.content.split_whitespace() {
                    let cleaned = word.trim_matches(|c| {
                        c == '(' || c == ')' || c == '<' || c == '>' || c == '"' || c == '\''
                    });
                    if (cleaned.starts_with("http://") || cleaned.starts_with("https://"))
                        && described_source_spec.is_none()
                    {
                        described_source_spec = Some((
                            "http".into(),
                            json!({
                                "type": "http",
                                "http": {
                                    "url": cleaned,
                                    "verb": "GET",
                                    "headers": { "Accept": "application/json" },
                                    "timeout": "15s"
                                }
                            }),
                            Vec::new(),
                        ));
                        break;
                    }
                }
            } else if name == ".env" || name.ends_with(".env") {
                // Parse .env
                let mut env_map = BTreeMap::new();
                for line in file.content.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    if let Some((k, v)) = trimmed.split_once('=') {
                        env_map.insert(
                            k.trim().to_string(),
                            v.trim().trim_matches('"').trim_matches('\'').to_string(),
                        );
                    }
                }
                let mut findings = Vec::new();
                if let Some(url_val) = env_map.get("URL") {
                    described_source_spec = Some((
                        "http".into(),
                        json!({
                            "type": "http",
                            "http": {
                                "url": url_val,
                                "verb": "GET",
                                "headers": { "Accept": "application/json" },
                                "timeout": "15s"
                            }
                        }),
                        findings,
                    ));
                } else if let (Some(host), Some(topic)) =
                    (env_map.get("MQTT_HOST"), env_map.get("MQTT_TOPIC"))
                {
                    let mut mqtt_spec = json!({
                        "urls": [host],
                        "topics": [topic]
                    });
                    for k in env_map.keys() {
                        if k.ends_with("_PASSWORD")
                            || k.ends_with("_SECRET")
                            || k.ends_with("_TOKEN")
                        {
                            let secret_name = format!(
                                "{space_name}-{}",
                                k.to_ascii_lowercase().replace('_', "-")
                            );
                            mqtt_spec["password"] = json!({
                                "secretRef": { "name": secret_name, "key": k }
                            });
                            findings.push(Finding {
                                level: Level::Warning,
                                path: "spec.mqtt.password".to_string(),
                                message: format!(
                                    "fill in the secret '{secret_name}' with key '{k}'"
                                ),
                            });
                        }
                    }
                    described_source_spec = Some((
                        "mqtt".into(),
                        json!({
                            "type": "mqtt",
                            "mqtt": mqtt_spec
                        }),
                        findings,
                    ));
                } else if let (Some(brokers), Some(topic)) =
                    (env_map.get("KAFKA_BROKERS"), env_map.get("KAFKA_TOPIC"))
                {
                    let broker_list: Vec<&str> = brokers.split(',').map(str::trim).collect();
                    let kafka_spec = json!({
                        "brokers": broker_list,
                        "topics": [topic]
                    });
                    described_source_spec = Some((
                        "kafka".into(),
                        json!({
                            "type": "kafka",
                            "kafka": kafka_spec
                        }),
                        findings,
                    ));
                }
            }
        }
    }

    let class_name = class_name_of(
        input.type_name.as_deref(),
        sample_bytes.as_ref().map(|(n, _)| n.as_str()),
        &space_name,
    );
    let description = input
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_owned);

    // 1. DataModel
    let has_datamodel = manifests.contains_key("DataModel");
    let mut inferred_model = false;
    let mut model_verdict = None;
    let mut model_manifest = manifests.get("DataModel").cloned();
    let mut model_linkml_text = linkml_source.clone();

    if !has_datamodel {
        let (inferred_text, infer_findings) = if let Some(text) = linkml_source.clone() {
            (text, Vec::new())
        } else if let Some((name, bytes)) = &sample_bytes {
            let fmt = if name.ends_with(".csv") {
                Some("csv")
            } else if name.ends_with(".json") {
                Some("json")
            } else {
                None
            };
            match model_tools::infer_schema_from_bytes(state, Some(&class_name), bytes, fmt).await {
                Ok(resp) => {
                    let text = resp
                        .get("linkml")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    (text, Vec::new())
                }
                Err(err) => {
                    let err_msg = err.to_string();
                    (
                        String::new(),
                        vec![Finding {
                            level: Level::Error,
                            path: "linkml".into(),
                            message: err_msg,
                        }],
                    )
                }
            }
        } else if let Some(url_str) = &input.url {
            // The person's URL is probed the way the data source Check probes it: one fetch on
            // the project's runner (MF-39), never from the Portal's own network.
            let spec = json!({ "type": "http", "http": { "url": url_str } });
            match crate::api::pipeline_test::probe_source(state, project, &spec).await {
                Some(probe) if probe.skipped.is_none() && probe.sample.is_some() => {
                    let sample = probe.sample.unwrap_or(Value::Null).to_string();
                    match model_tools::infer_schema_from_bytes(
                        state,
                        Some(&class_name),
                        sample.as_bytes(),
                        Some("json"),
                    )
                    .await
                    {
                        Ok(res) => (
                            res.get("linkml")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            Vec::new(),
                        ),
                        Err(e) => (
                            String::new(),
                            vec![Finding {
                                level: Level::Error,
                                path: "linkml".into(),
                                message: e.to_string(),
                            }],
                        ),
                    }
                }
                Some(probe) => (
                    String::new(),
                    vec![Finding {
                        level: Level::Error,
                        path: "url".into(),
                        message: probe
                            .skipped
                            .unwrap_or_else(|| "the feed answered no sample".to_string()),
                    }],
                ),
                None => (
                    String::new(),
                    vec![Finding {
                        level: Level::Error,
                        path: "url".into(),
                        message: "the URL is not an http(s) URL".to_string(),
                    }],
                ),
            }
        } else {
            (
                String::new(),
                vec![Finding {
                    level: Level::Error,
                    path: "linkml".into(),
                    message: "no sample file or url to infer schema from".into(),
                }],
            )
        };

        let inferred_text = match &description {
            Some(description) if !inferred_text.is_empty() => {
                describe_class(&inferred_text, &class_name, description)
            }
            _ => inferred_text,
        };
        model_linkml_text = Some(inferred_text.clone());
        let classes_array = json!([class_name]);
        let manifest = json!({
            "apiVersion": API_VERSION,
            "kind": "DataModel",
            "metadata": {
                "name": space_name,
                "namespace": project,
                "title": { "en": format!("{} data model", words_capitalized(&space_name)) }
            },
            "spec": {
                "contextSpaceRef": space_name,
                "linkml": format!("./{space_name}.linkml.yaml"),
                "version": "0.1.0",
                "lifecycle": "draft",
                "classes": classes_array,
                "source": inferred_text
            }
        });

        let is_ok = infer_findings.is_empty()
            && !inferred_text.is_empty()
            && inferred_text.contains("classes:");
        model_verdict = Some(Verdict::new(is_ok, infer_findings, None, &manifest));
        let mut manifest = manifest;
        describe(&mut manifest, description.as_deref());
        model_manifest = Some(manifest);
        inferred_model = true;
    }

    // 2. ContextSpace
    let has_space = manifests.contains_key("ContextSpace");
    let mut inferred_space = false;
    let mut space_manifest = manifests.get("ContextSpace").cloned();
    if !has_space {
        let manifest = json!({
            "apiVersion": API_VERSION,
            "kind": "ContextSpace",
            "metadata": {
                "name": space_name,
                "namespace": project,
                "title": { "en": words_capitalized(&space_name) }
            },
            "spec": {
                "isSandbox": false,
                "defaultLocale": "en",
                "dataModelRef": {
                    "kind": "DataModel",
                    "name": space_name
                }
            }
        });
        let mut manifest = manifest;
        describe(&mut manifest, description.as_deref());
        space_manifest = Some(manifest);
        inferred_space = true;
    }

    // 3. DataSource
    let has_datasource = manifests.contains_key("DataSource");
    let mut inferred_datasource = false;
    let mut datasource_manifest = manifests.get("DataSource").cloned();
    let mut ds_findings = Vec::new();

    if !has_datasource {
        if let Some((ds_type, spec, findings)) = described_source_spec {
            let mut ds_spec = spec;
            ds_spec["type"] = Value::String(ds_type);
            let manifest = json!({
                "apiVersion": API_VERSION,
                "kind": "DataSource",
                "metadata": {
                    "name": format!("{space_name}-source"),
                    "namespace": project,
                    "title": { "en": format!("{} source", words_capitalized(&space_name)) }
                },
                "spec": ds_spec
            });
            let mut manifest = manifest;
            describe(&mut manifest, description.as_deref());
            datasource_manifest = Some(manifest);
            ds_findings = findings;
            inferred_datasource = true;
        }
    }

    // 4. Pipeline, and the Endpoint it writes through: a dropped one, the one the mirror
    // already serves for the space, or a drafted organization-wide one (EP-02 slug).
    let has_pipeline = manifests.contains_key("Pipeline");
    let mut inferred_pipeline = false;
    let mut pipeline_manifest = manifests.get("Pipeline").cloned();
    let mut inferred_endpoint = false;
    let mut endpoint_manifest = manifests.get("Endpoint").cloned();

    if !has_pipeline && datasource_manifest.is_some() {
        let ds_name = datasource_manifest
            .as_ref()
            .and_then(|m| m.pointer("/metadata/name"))
            .and_then(Value::as_str)
            .unwrap_or("source");
        let mapping = bloblang_content.unwrap_or_else(|| {
            format!(
                "root = this\nroot.id = \"urn:ngsi-ld:{class_name}:\" + (this.stationId | this.id | this.station_id | uuid_v4()).string()\nroot.type = \"{class_name}\"\n"
            )
        });
        let org = crate::api::assistant::org_domain(state, project);
        let endpoint_name = endpoint_manifest
            .as_ref()
            .and_then(|m| m.pointer("/metadata/name"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                state
                    .mirror
                    .list(project, "Endpoint", &crate::store::ListOptions::default())
                    .items
                    .into_iter()
                    .find(|ep| {
                        ep.spec.get("contextSpaceRef").and_then(Value::as_str) == Some(&space_name)
                    })
                    .map(|ep| ep.metadata.name)
            })
            .unwrap_or_else(|| {
                let name = format!("{space_name}-all");
                endpoint_manifest = Some(json!({
                    "apiVersion": API_VERSION,
                    "kind": "Endpoint",
                    "metadata": {
                        "name": name,
                        "namespace": project,
                        "title": { "en": format!("{} context, everything", words_capitalized(&space_name)) }
                    },
                    "spec": {
                        "contextSpaceRef": space_name,
                        "slug": crate::agents::share::slug(),
                        "audience": "organization",
                        "enabledRepresentations": ["ngsi-ld"]
                    }
                }));
                inferred_endpoint = true;
                name
            });
        let target_endpoint = format!("urn:ngsi-ld:Endpoint:{org}:{space_name}:{endpoint_name}");

        let spec = json!({
            "class": "auto",
            "period": "60s",
            "targetEndpoint": target_endpoint,
            "source": {
                "dataSourceRef": {
                    "kind": "DataSource",
                    "name": ds_name
                }
            },
            "output": {
                "type": class_name,
                "mode": "upsert"
            },
            "compute": {
                "kind": "bloblang",
                "bloblang": mapping
            },
            "quotas": {
                "maxMemoryMb": 64,
                "cpuMillicores": 100
            }
        });
        let manifest = json!({
            "apiVersion": API_VERSION,
            "kind": "Pipeline",
            "metadata": {
                "name": format!("{space_name}-load"),
                "namespace": project,
                "title": { "en": format!("{} pipeline", words_capitalized(&space_name)) }
            },
            "spec": spec
        });
        pipeline_manifest = Some(manifest);
        inferred_pipeline = true;
    }

    // Now compute verdicts and store drafts
    let mut drafts = Vec::new();
    let store = draft_store(state);

    // Save DataModel draft
    if let Some(m) = model_manifest {
        let name = m
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or(&space_name);
        let stored = store
            .put(
                project,
                "DataModel",
                name,
                m.clone(),
                None,
                &caller.identity.username,
                caller.via.touched_kind(),
            )
            .await
            .map_err(draft_error)?;
        let verdict = if let Some(v) = model_verdict {
            store
                .set_verdict(project, "DataModel", name, v)
                .await
                .map_err(draft_error)?
                .verdict
        } else {
            stored.verdict
        };
        drafts.push(CompletedDraft {
            kind: "DataModel".into(),
            name: name.into(),
            inferred: inferred_model,
            manifest: m,
            verdict,
        });
    }

    // Save ContextSpace draft
    if let Some(m) = space_manifest {
        let name = m
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or(&space_name);
        let stored = store
            .put(
                project,
                "ContextSpace",
                name,
                m.clone(),
                None,
                &caller.identity.username,
                caller.via.touched_kind(),
            )
            .await
            .map_err(draft_error)?;
        // Dry run space
        let dry_res = ops::call(
            ops::find("jc_manifest_dry_run").unwrap(),
            caller,
            state,
            project,
            json!({ "manifest": m }),
        )
        .await;
        let v = match dry_res {
            Ok(val) => serde_json::from_value::<Verdict>(
                val.get("verdict").cloned().unwrap_or(Value::Null),
            )
            .ok(),
            Err(e) => Some(Verdict::red(
                &m,
                vec![Finding {
                    level: Level::Error,
                    path: "".into(),
                    message: e.to_string(),
                }],
                None,
            )),
        };
        let final_v = if let Some(verdict) = v {
            store
                .set_verdict(project, "ContextSpace", name, verdict)
                .await
                .map_err(draft_error)?
                .verdict
        } else {
            stored.verdict
        };
        drafts.push(CompletedDraft {
            kind: "ContextSpace".into(),
            name: name.into(),
            inferred: inferred_space,
            manifest: m,
            verdict: final_v,
        });
    }

    // Save DataSource draft
    if let Some(m) = datasource_manifest {
        let name = m
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or("source");
        let stored = store
            .put(
                project,
                "DataSource",
                name,
                m.clone(),
                None,
                &caller.identity.username,
                caller.via.touched_kind(),
            )
            .await
            .map_err(draft_error)?;

        let check_res = if input.url.is_some() {
            ops::call(
                ops::find("jc_datasource_check").unwrap(),
                caller,
                state,
                project,
                json!({ "manifest": m }),
            )
            .await
        } else {
            ops::call(
                ops::find("jc_manifest_dry_run").unwrap(),
                caller,
                state,
                project,
                json!({ "manifest": m }),
            )
            .await
        };
        let v = match check_res {
            Ok(val) => {
                let mut verdict: Verdict =
                    serde_json::from_value(val.get("verdict").cloned().unwrap_or(Value::Null))
                        .unwrap_or_else(|_| Verdict::green(&m, None));
                verdict.findings.extend(ds_findings);
                Some(verdict)
            }
            Err(e) => {
                let mut findings = ds_findings;
                findings.push(Finding {
                    level: Level::Error,
                    path: "".into(),
                    message: e.to_string(),
                });
                Some(Verdict::red(&m, findings, None))
            }
        };
        let final_v = if let Some(verdict) = v {
            store
                .set_verdict(project, "DataSource", name, verdict)
                .await
                .map_err(draft_error)?
                .verdict
        } else {
            stored.verdict
        };
        drafts.push(CompletedDraft {
            kind: "DataSource".into(),
            name: name.into(),
            inferred: inferred_datasource,
            manifest: m,
            verdict: final_v,
        });
    }

    // Save Endpoint draft
    if let Some(m) = endpoint_manifest {
        let name = m
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or(&space_name);
        let stored = store
            .put(
                project,
                "Endpoint",
                name,
                m.clone(),
                None,
                &caller.identity.username,
                caller.via.touched_kind(),
            )
            .await
            .map_err(draft_error)?;
        let dry_res = ops::call(
            ops::find("jc_manifest_dry_run").unwrap(),
            caller,
            state,
            project,
            json!({ "manifest": m }),
        )
        .await;
        let v = match dry_res {
            Ok(val) => serde_json::from_value::<Verdict>(
                val.get("verdict").cloned().unwrap_or(Value::Null),
            )
            .ok(),
            Err(e) => Some(Verdict::red(
                &m,
                vec![Finding {
                    level: Level::Error,
                    path: "".into(),
                    message: e.to_string(),
                }],
                None,
            )),
        };
        let final_v = if let Some(verdict) = v {
            store
                .set_verdict(project, "Endpoint", name, verdict)
                .await
                .map_err(draft_error)?
                .verdict
        } else {
            stored.verdict
        };
        drafts.push(CompletedDraft {
            kind: "Endpoint".into(),
            name: name.into(),
            inferred: inferred_endpoint,
            manifest: m,
            verdict: final_v,
        });
    }

    // Save Pipeline draft
    if let Some(m) = pipeline_manifest {
        let name = m
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or("pipeline");
        let stored = store
            .put(
                project,
                "Pipeline",
                name,
                m.clone(),
                None,
                &caller.identity.username,
                caller.via.touched_kind(),
            )
            .await
            .map_err(draft_error)?;

        let p_verdict = if let Some((sample_filename, sample_bytes_vec)) = &sample_bytes {
            let sample_text = String::from_utf8_lossy(sample_bytes_vec).to_string();
            let sample_fmt = if sample_filename.ends_with(".csv") {
                SampleFormat::Csv
            } else if sample_filename.ends_with(".json") {
                SampleFormat::Json
            } else {
                SampleFormat::Text
            };
            let test_input = json!({
                "pipeline": m,
                "sample": Sample {
                    text: Some(sample_text),
                    url: None,
                    format: sample_fmt,
                }
            });
            let test_res = ops::call(
                ops::find("jc_pipeline_test").unwrap(),
                caller,
                state,
                project,
                test_input,
            )
            .await;
            match test_res {
                Ok(val) => serde_json::from_value::<Verdict>(
                    val.get("verdict").cloned().unwrap_or(Value::Null),
                )
                .ok(),
                Err(e) => Some(Verdict::red(
                    &m,
                    vec![Finding {
                        level: Level::Error,
                        path: "".into(),
                        message: e.to_string(),
                    }],
                    None,
                )),
            }
        } else if let Some(url_str) = &input.url {
            let test_input = json!({
                "pipeline": m,
                "sample": Sample {
                    text: None,
                    url: Some(url_str.clone()),
                    format: SampleFormat::Json,
                }
            });
            let test_res = ops::call(
                ops::find("jc_pipeline_test").unwrap(),
                caller,
                state,
                project,
                test_input,
            )
            .await;
            match test_res {
                Ok(val) => serde_json::from_value::<Verdict>(
                    val.get("verdict").cloned().unwrap_or(Value::Null),
                )
                .ok(),
                Err(e) => Some(Verdict::red(
                    &m,
                    vec![Finding {
                        level: Level::Error,
                        path: "".into(),
                        message: e.to_string(),
                    }],
                    None,
                )),
            }
        } else {
            Some(Verdict::red(
                &m,
                vec![Finding {
                    level: Level::Error,
                    path: "".into(),
                    message: "no sample to test the mapping on".into(),
                }],
                None,
            ))
        };

        let final_v = if let Some(verdict) = p_verdict {
            store
                .set_verdict(project, "Pipeline", name, verdict)
                .await
                .map_err(draft_error)?
                .verdict
        } else {
            stored.verdict
        };
        drafts.push(CompletedDraft {
            kind: "Pipeline".into(),
            name: name.into(),
            inferred: inferred_pipeline,
            manifest: m,
            verdict: final_v,
        });
    }

    let propose_ready = !drafts.is_empty()
        && drafts.iter().all(|d| {
            d.verdict
                .as_ref()
                .is_some_and(|v| v.is_fresh_for(&d.manifest))
        });

    let lane = riskiest_lane(&drafts);

    let change = if input.propose {
        if !propose_ready {
            let failing_check = drafts
                .iter()
                .find(|d| {
                    d.verdict
                        .as_ref()
                        .is_none_or(|v| !v.is_fresh_for(&d.manifest))
                })
                .map(|d| match d.kind.as_str() {
                    "DataModel" => "jc_model_propose",
                    "DataSource" => "jc_datasource_check",
                    "Pipeline" => "jc_pipeline_test",
                    _ => "jc_manifest_dry_run",
                })
                .unwrap_or("jc_manifest_dry_run");
            return Err(OpError::Conflict(json!({
                "error": "verdict_required",
                "check": failing_check,
                "reason": "propose_not_ready"
            })));
        }

        // Commit bundle
        let mut bundle_files = Vec::new();
        let mut created = Vec::new();

        for d in &drafts {
            let committed = committed(&d.manifest);
            let info = resource::by_kind(&d.kind).ok_or_else(|| {
                OpError::Api(ApiError::BadRequest(format!("unknown kind '{}'", d.kind)))
            })?;
            let path = resource::repository_path(info, project, Some(&space_name), &d.name)
                .map_err(ApiError::BadRequest)?;
            let yaml_text = serde_yaml_ng::to_string(&committed)
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            bundle_files.push((path, yaml_text));
            created.push(d.name.clone());
        }

        if let Some(linkml) = model_linkml_text {
            let linkml_path = format!(
                "projects/{project}/spaces/{space_name}/datamodels/{space_name}.linkml.yaml"
            );
            bundle_files.push((linkml_path, linkml));
        }

        let report = ImportReport {
            created,
            replaced: Vec::new(),
            skipped: Vec::new(),
            renamed: BTreeMap::new(),
            native_files: if drafts.iter().any(|d| d.kind == "DataModel") {
                1
            } else {
                0
            },
            lane,
            source: Some("jc_space_complete".into()),
        };

        let ch = crate::api::import::propose_bundle(
            state,
            &caller.identity,
            project,
            report,
            bundle_files,
            Some(("ContextSpace", &space_name)),
        )
        .await?;
        Some(ch)
    } else {
        None
    };

    let out = SpaceCompleteOutput {
        space: space_name,
        found: found_kinds,
        drafts,
        propose_ready,
        lane,
        change,
    };

    Ok(serde_json::to_value(out)?)
}

/// The manifest as the repository takes it: no `status`, and no inline `spec.source` on a
/// DataModel (the draft carries the LinkML text for the editor; the repository gets it as the
/// `spec.linkml` file beside the manifest, which is what the kind validates, DM-56).
fn committed(manifest: &Value) -> Value {
    let mut committed = manifest.clone();
    if let Some(obj) = committed.as_object_mut() {
        obj.remove("status");
    }
    if committed["kind"] == "DataModel" {
        if let Some(spec) = committed["spec"].as_object_mut() {
            spec.remove("source");
        }
    }
    committed
}

fn riskiest_lane(drafts: &[CompletedDraft]) -> Lane {
    let mut lane = Lane::Green;
    for d in drafts {
        let l = match d.kind.as_str() {
            "DataModel" | "ContextSpace" | "DataSource" => Lane::Yellow,
            "Pipeline" => Lane::Green,
            // A public Endpoint is the Red lane (CC-19); an organization-wide one is Yellow.
            "Endpoint" if d.manifest["spec"]["audience"] == "public" => Lane::Red,
            _ => Lane::Yellow,
        };
        lane = match (lane, l) {
            (Lane::Red, _) | (_, Lane::Red) => Lane::Red,
            (Lane::Yellow, _) | (_, Lane::Yellow) => Lane::Yellow,
            _ => Lane::Green,
        };
    }
    lane
}

fn draft_error(err: ops::drafts::DraftError) -> OpError {
    match err {
        ops::drafts::DraftError::Conflict { current } => OpError::Conflict(json!({
            "type": "https://joinedcontext.com/problems/draft-conflict",
            "error": "draft_conflict",
            "current": current,
        })),
        ops::drafts::DraftError::Secret(path) => OpError::Api(ApiError::BadRequest(format!(
            "literal secret in field '{path}' is forbidden; use secretRef instead (MF-24)"
        ))),
        ops::drafts::DraftError::NotFound {
            project,
            kind,
            name,
        } => OpError::Api(ApiError::NotFound(format!(
            "draft '{kind}/{name}' not found in project '{project}'"
        ))),
        ops::drafts::DraftError::Db(msg) => OpError::Api(ApiError::Internal(msg)),
    }
}

#[cfg(test)]
mod tests {
    use super::{class_name_of, describe, describe_class, is_type_name};
    use serde_json::json;

    #[test]
    fn the_class_is_the_named_type_else_the_sample_else_the_space_never_entity() {
        assert_eq!(
            class_name_of(Some("BikeStation"), None, "bikes"),
            "BikeStation"
        );
        assert_eq!(
            class_name_of(Some("bad name"), None, "helsinki-bikes"),
            "HelsinkiBike"
        );
        assert_eq!(
            class_name_of(None, Some("stations.csv"), "bikes"),
            "Station"
        );
        assert_eq!(class_name_of(None, None, "helsinki-bikes"), "HelsinkiBike");
        assert_eq!(class_name_of(None, None, "entity"), "FeedEntity");
        assert_eq!(class_name_of(None, None, "7-feeds"), "Feed7Feed");
        assert!(is_type_name("WeatherObserved"));
        assert!(!is_type_name("weatherObserved"));
        assert!(!is_type_name("Weather-Observed"));
        assert!(!is_type_name(""));
    }

    #[test]
    fn a_description_lands_on_the_inferred_class_unless_it_has_one() {
        let source = "id: https://example.com/bikes\nclasses:\n  BikeStation:\n    slots: [name]\n";
        let described = describe_class(source, "BikeStation", "Docking stations of city bikes");
        let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str(&described).unwrap();
        assert_eq!(
            doc["classes"]["BikeStation"]["description"].as_str(),
            Some("Docking stations of city bikes")
        );
        assert_eq!(
            describe_class(&described, "BikeStation", "other"),
            described
        );
        assert_eq!(
            describe_class("not: [yaml", "BikeStation", "x"),
            "not: [yaml"
        );
        let mut manifest = json!({ "metadata": { "name": "bikes" } });
        describe(&mut manifest, Some("  City bikes  "));
        assert_eq!(
            manifest["metadata"]["description"],
            json!({ "en": "City bikes" })
        );
        describe(&mut manifest, Some("else"));
        assert_eq!(
            manifest["metadata"]["description"],
            json!({ "en": "City bikes" })
        );
    }

    #[test]
    fn a_committed_datamodel_keeps_its_linkml_path_and_loses_the_inline_source() {
        let draft = serde_json::json!({
            "kind": "DataModel",
            "spec": { "linkml": "./bikes.linkml.yaml", "source": "classes: {}" },
            "status": { "x": 1 }
        });
        let out = super::committed(&draft);
        assert_eq!(out["spec"]["linkml"], "./bikes.linkml.yaml");
        assert!(out["spec"].get("source").is_none());
        assert!(out.get("status").is_none());
        let space = serde_json::json!({ "kind": "ContextSpace", "spec": { "source": "kept" } });
        assert_eq!(super::committed(&space)["spec"]["source"], "kept");
    }
}
