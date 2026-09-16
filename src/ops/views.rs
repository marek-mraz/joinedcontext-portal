//! The reads the Portal serves that had no operation (AG-59, CC-48, T-0840).
//!
//! A route the registry does not know is a defect: what a browser can read through the Portal,
//! an MCP client and the assistant read through the operation behind the same function. These
//! are the read-only ones — a pipeline's counters, what happened in the project, the federation
//! of a project, and a data model's LinkML — each calling exactly what its route calls.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{parse_input, Annotations, Caller, OpError, Operation};
use crate::change::Lane;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NamedInput {
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActivityInput {
    #[serde(default)]
    pub space: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

fn named_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "name": { "type": "string", "description": "The resource's name" } },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn nothing_input_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn metrics_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "What the runner has counted for this pipeline since it started (PL-46)",
        "properties": {
            "pipeline": { "type": "string" },
            "state": { "type": "string" },
            "received": { "type": "integer" },
            "sent": { "type": "integer" },
            "errors": { "type": "integer" },
            "messagesPerSecond": { "type": "number" },
            "latencyP99Ms": { "type": "number" },
            "lastError": { "type": "string" }
        }
    })
}

fn activity_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "space": { "type": "string" },
            "kind": { "type": "string", "description": "One or more event kinds, comma-separated" },
            "source": { "type": "string" },
            "severity": { "type": "string", "enum": ["info", "warning", "error"], "description": "This severity and everything above it" },
            "since": { "type": "string", "format": "date-time" },
            "object": { "type": "string", "description": "One object the events belong to, as {plural}/{name}" },
            "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
            "cursor": { "type": "string" }
        },
        "additionalProperties": false
    })
}

fn activity_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "time": { "type": "string", "format": "date-time" },
                        "project": { "type": "string" },
                        "space": { "type": "string" },
                        "kind": { "type": "string" },
                        "source": { "type": "string" },
                        "summary": { "type": "string" },
                        "severity": { "type": "string" },
                        "details": { "type": "object" }
                    }
                }
            },
            "next": { "type": "string", "description": "The cursor of the next page, when there is one" }
        },
        "required": ["items"]
    })
}

fn graph_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The project's spaces, endpoints, pipelines, apps, catalogues and registrations, and what links them (EP-58)",
        "properties": {
            "nodes": { "type": "array", "items": { "type": "object" } },
            "edges": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["nodes", "edges"]
    })
}

fn source_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "source": { "type": "string", "description": "The model's LinkML, as the repository holds it" }
        },
        "required": ["name", "source"]
    })
}

/// A read needs the caller's `read` on the kind, the way every other door reads (PF-59). A kind
/// no binding of theirs reads is not there, never refused by name (R20).
fn readable(caller: &Caller, state: &AppState, project: &str, kind: &str) -> Result<(), OpError> {
    if crate::permissions::for_request(state, &caller.identity, project).may_read(kind) {
        return Ok(());
    }
    Err(OpError::Api(crate::error::ApiError::NotFound(format!(
        "kind '{kind}' not found in project '{project}'"
    ))))
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_pipeline_metrics",
            title: "Read Pipeline Counters",
            description: "What the runner has counted for one pipeline: state, messages, errors and latency",
            input: named_input_schema,
            output: metrics_output_schema,
            annotations: Annotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<NamedInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NamedInput = parse_input(val)?;
                    readable(caller, state, project, "Pipeline")?;
                    let metrics =
                        crate::api::pipelines::metrics_for(state, project, &input.name).await?;
                    Ok(serde_json::to_value(metrics)?)
                })
            },
        },
        Operation {
            name: "jc_activity_list",
            title: "List Activity",
            description: "What happened in the project: the same events, filters and paging the activity page reads",
            input: activity_input_schema,
            output: activity_output_schema,
            annotations: Annotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ActivityInput>(val.clone()).map(|_| ()),
            run: |_, state, project, val| {
                Box::pin(async move {
                    let input: ActivityInput = parse_input(val)?;
                    let query = crate::api::activity::ActivityQuery {
                        space: input.space,
                        kind: input.kind,
                        source: input.source,
                        severity: input.severity,
                        since: input.since,
                        object: input.object,
                        limit: input.limit,
                        cursor: input.cursor,
                    };
                    let filter = query.into_filter()?;
                    let page = state
                        .activity
                        .list(project, &filter)
                        .await
                        .map_err(|err| crate::error::ApiError::Unavailable(err.to_string()))?;
                    Ok(json!({ "items": page.items, "next": page.next }))
                })
            },
        },
        Operation {
            name: "jc_federation_graph",
            title: "Read Federation",
            description: "The project's spaces, endpoints, pipelines and apps, and what links them",
            input: nothing_input_schema,
            output: graph_output_schema,
            annotations: Annotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<EmptyInput>(val.clone()).map(|_| ()),
            run: |_, state, project, val| {
                Box::pin(async move {
                    let _: EmptyInput = parse_input(val)?;
                    Ok(serde_json::to_value(crate::api::federation::graph_of(
                        state, project,
                    ))?)
                })
            },
        },
        Operation {
            name: "jc_model_source_get",
            title: "Read Model Source",
            description: "One DataModel's LinkML, as the repository holds it",
            input: named_input_schema,
            output: source_output_schema,
            annotations: Annotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "DataModel",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<NamedInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NamedInput = parse_input(val)?;
                    readable(caller, state, project, "DataModel")?;
                    // A manifest that carries its LinkML is read from the manifest, as every
                    // editor reads it; one that names a file is read from the repository.
                    let inline = state
                        .mirror
                        .get(project, "DataModel", &input.name)
                        .and_then(|envelope| {
                            envelope
                                .spec
                                .get("linkml")
                                .and_then(Value::as_str)
                                .filter(|text| text.contains('\n'))
                                .map(str::to_owned)
                        });
                    let source = match inline {
                        Some(text) => text,
                        None => {
                            crate::api::datamodels::read_source(state, project, &input.name).await?
                        }
                    };
                    Ok(json!({ "name": input.name, "source": source }))
                })
            },
        },
    ]
}
