//! Every kind read, changed and removed through the registry (AG-77, ADR-N-021): the functions
//! behind the resource routes, so an MCP client, the assistant and the Portal's forms reach the
//! same Verdict and the same Change.

use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    change_schema, manifest_input_schema, parse_input, propose_with_optional_draft, Annotations,
    Caller, ManifestInput, OpError, Operation, Via,
};
use crate::api::assistant::ref_name;
use crate::api::changes;
use crate::api::delete::{self, DeleteOutcome};
use crate::api::mutate::ProposeOutcome;
use crate::change::Lane;
use crate::error::ApiError;
use crate::resource::{self, KindInfo, ResourceEnvelope};
use crate::state::AppState;
use crate::store::ListOptions;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceListInput {
    pub kind: String,
    #[serde(default)]
    pub space: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGetInput {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDeleteInput {
    pub kind: String,
    pub name: String,
    /// The resource's name typed back (CC-39).
    pub confirm: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRejectInput {
    pub id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// The operations whose function is the REST route's own and checks the caller as the route does:
/// reading needs a signed-in person, proposing and deleting the verb on the kind (AG-77, PF-50).
pub const CHECKED_BY_THE_ROUTE: [&str; 4] = [
    "jc_resource_list",
    "jc_resource_get",
    "jc_resource_propose",
    "jc_resource_delete",
];

/// The catalogue row of a kind an input names; an unknown kind is answered with the kinds there are.
fn kind_named(kind: &str) -> Result<&'static KindInfo, OpError> {
    resource::by_kind(kind).ok_or_else(|| OpError::InvalidInput {
        path: "/kind".into(),
        message: format!(
            "'{kind}' is not a kind of this Portal; the kinds are {}",
            resource::kinds()
                .map(|info| info.kind)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

/// The context space a resource belongs to: its space label, else the space its spec references.
fn space_of(envelope: &ResourceEnvelope) -> Option<String> {
    envelope
        .metadata
        .labels
        .get("joinedcontext.com/space")
        .cloned()
        .or_else(|| ref_name(&envelope.spec["contextSpaceRef"]))
}

/// One row of a listing: enough to pick a resource and open it, not the whole manifest.
fn row(envelope: &ResourceEnvelope) -> Value {
    let status = envelope.status.as_ref();
    json!({
        "kind": envelope.kind,
        "name": envelope.metadata.name,
        "space": space_of(envelope),
        "title": envelope.metadata.title,
        "phase": status.map(|status| status.phase),
        "sourceUrl": status.and_then(|status| status.source_url.clone()),
    })
}

fn list_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string", "description": "The manifest kind, e.g. Pipeline" },
            "space": { "type": "string", "description": "Only the resources of this context space" }
        },
        "required": ["kind"],
        "additionalProperties": false
    })
}

fn list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": { "type": "array", "items": { "type": "object" } }
        }
    })
}

fn get_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string" },
            "name": { "type": "string" }
        },
        "required": ["kind", "name"],
        "additionalProperties": false
    })
}

fn get_output_schema() -> Value {
    json!({ "type": "object", "description": "The manifest with its status" })
}

fn delete_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string" },
            "name": { "type": "string" },
            "confirm": { "type": "string", "description": "The resource's name typed back" }
        },
        "required": ["kind", "name", "confirm"],
        "additionalProperties": false
    })
}

fn reject_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "reason": { "type": "string" }
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

async fn list(state: &AppState, project: &str, input: ResourceListInput) -> Result<Value, OpError> {
    let info = kind_named(&input.kind)?;
    let page = state
        .mirror
        .list(project, info.kind, &ListOptions::default());
    let items: Vec<Value> = page
        .items
        .iter()
        .filter(|envelope| {
            input
                .space
                .as_deref()
                .is_none_or(|space| space_of(envelope).as_deref() == Some(space))
        })
        .map(row)
        .collect();
    Ok(json!({ "items": items }))
}

async fn get(state: &AppState, project: &str, input: ResourceGetInput) -> Result<Value, OpError> {
    let info = kind_named(&input.kind)?;
    match state.mirror.get(project, info.kind, &input.name) {
        Some(envelope) => Ok(serde_json::to_value(envelope)?),
        None => {
            let mut names: Vec<String> = state
                .mirror
                .list(project, info.kind, &ListOptions::default())
                .items
                .into_iter()
                .map(|envelope| envelope.metadata.name)
                .collect();
            names.sort();
            Err(OpError::Api(ApiError::NotFound(if names.is_empty() {
                format!(
                    "{} '{}' not found in project '{project}', which has none",
                    info.kind, input.name
                )
            } else {
                format!(
                    "{} '{}' not found in project '{project}'; it has {}",
                    info.kind,
                    input.name,
                    names.join(", ")
                )
            })))
        }
    }
}

async fn propose(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: ManifestInput,
) -> Result<Value, OpError> {
    let kind = match (&input.manifest, &input.draft) {
        (Some(manifest), _) => manifest
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        (None, Some(draft)) => draft.kind.clone(),
        (None, None) => {
            return Err(OpError::InvalidInput {
                path: "/manifest".into(),
                message: "either manifest or draft is required".into(),
            })
        }
    };
    let info = kind_named(&kind)?;
    propose_with_optional_draft(
        caller,
        state,
        project,
        info.plural,
        "jc_manifest_dry_run",
        input,
    )
    .await
}

async fn remove(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: ResourceDeleteInput,
) -> Result<Value, OpError> {
    let info = kind_named(&input.kind)?;
    if input.confirm != input.name {
        return Err(OpError::Api(ApiError::BadRequest(format!(
            "confirm must be the resource's name, '{}' (CC-39)",
            input.name
        ))));
    }
    match delete::delete_with_identity(
        &caller.identity,
        state,
        project,
        info.plural,
        &input.name,
        false,
    )
    .await?
    {
        DeleteOutcome::Proposed(change) => Ok(ProposeOutcome::Change(change).into_value()),
        DeleteOutcome::DryRun(result) => Ok(ProposeOutcome::DryRun(result).into_value()),
        DeleteOutcome::Referenced { here, elsewhere } => Err(OpError::Conflict(json!({
            "error": "referenced",
            "detail": DeleteOutcome::conflict_message(&here, elsewhere),
            "by": here,
            "elsewhere": elsewhere,
        }))),
    }
}

async fn reject(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: ChangeRejectInput,
) -> Result<Value, OpError> {
    let change = changes::reject_change_for(
        state,
        &caller.identity,
        project,
        &input.id,
        input.reason.as_deref(),
    )
    .await?;
    Ok(ProposeOutcome::Change(change).into_value())
}

/// An agent run never decides a change (AG-11): approving and rejecting are a person's.
pub fn refuse_agent(caller: &Caller) -> Result<(), OpError> {
    if caller.via == Via::Agent {
        return Err(OpError::Api(ApiError::Denied(
            "an agent never approves or rejects a change; a person does (AG-11)".into(),
        )));
    }
    Ok(())
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_resource_list",
            title: "List Resources",
            description: "Lists the resources of one kind in the project, optionally of one context space",
            input: list_input_schema,
            output: list_output_schema,
            annotations: Annotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ResourceListInput>(val.clone()).map(|_| ()),
            run: |_, state, project, val| {
                Box::pin(async move { list(state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: "jc_resource_get",
            title: "Get Resource",
            description: "Reads one resource's manifest and status by kind and name",
            input: get_input_schema,
            output: get_output_schema,
            annotations: Annotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ResourceGetInput>(val.clone()).map(|_| ()),
            run: |_, state, project, val| {
                Box::pin(async move { get(state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: "jc_resource_propose",
            title: "Propose Resource",
            description: "Proposes creating or changing a resource of any kind from its manifest or a draft; the change waits for a person's approval",
            input: manifest_input_schema,
            output: change_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            lane: Lane::Yellow,
            validate: |val| parse_input::<ManifestInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { propose(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: "jc_resource_delete",
            title: "Delete Resource",
            description: "Proposes removing a resource by kind and name, its name typed back; refused while other resources reference it",
            input: delete_input_schema,
            output: change_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            lane: Lane::Red,
            validate: |val| parse_input::<ResourceDeleteInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { remove(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: "jc_change_reject",
            title: "Reject Change",
            description: "Rejects a change proposal with a reason and closes its merge request",
            input: reject_input_schema,
            output: change_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Change",
            verb: Some(Verb::Approve),
            lane: Lane::Yellow,
            validate: |val| parse_input::<ChangeRejectInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller)?;
                    reject(caller, state, project, parse_input(val)?).await
                })
            },
        },
    ]
}
