//! The agent runs, through the registry (AG-59, CC-48, T-0840).
//!
//! Starting an application, cancelling a run and publishing what it built had a route and no
//! operation, so an MCP client could ask the Portal for anything except the one thing the
//! Portal is for. Each operation calls the route's own handler, so the checks, the quotas and
//! the answers are the route's: nothing here decides anything of its own.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{parse_input, Annotations, Caller, OpError, Operation};
use crate::auth::session::{CurrentUser, Session};
use crate::change::Lane;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunIdInput {
    pub id: String,
}

fn run_id_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "id": { "type": "string", "description": "The run's id" } },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn create_input_schema() -> Value {
    json!({
        "type": "object",
        "description": "What an unattended run is given (AG-69, AP-44); the same body the route takes",
        "properties": {
            "appName": { "type": "string" },
            "endpointName": { "type": "string" },
            "endpointNames": { "type": "array", "items": { "type": "string" } },
            "profile": { "type": "string" },
            "appClass": { "type": "string" },
            "visibility": { "type": "string" },
            "prompt": { "type": "string" },
            "kind": { "type": "string", "enum": ["application", "dashboard", "analysis", "conversation"] },
            "unattended": { "type": "boolean" },
            "continues": { "type": "string" },
            "dataNeeds": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["appName", "prompt", "dataNeeds"]
    })
}

fn run_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "project": { "type": "string" },
            "appName": { "type": "string" },
            "kind": { "type": "string" },
            "status": { "type": "string" },
            "createdBy": { "type": "string" },
            "createdAt": { "type": "string", "format": "date-time" }
        },
        "required": ["id", "status"]
    })
}

/// The caller as the route's handlers read them.
///
/// The operations run behind the same permission checks the routes make, and those read a
/// session; this is that identity in the shape they take. Nothing is widened: the tokens and
/// the ticket stay where they were, and an agent is refused above by [`refuse_agent`].
fn as_user(caller: &Caller) -> CurrentUser {
    let now = crate::auth::session::now_unix();
    CurrentUser(Session {
        identity: caller.identity.clone(),
        expires_at: now + 60,
        issued_at: now,
        id_token: String::new(),
        access_expires_at: now + 60,
        refresh_token: None,
    })
}

/// A run acts for the person who started it; an agent does not start another run of its own
/// (AG-11, AG-70).
fn refuse_agent(caller: &Caller) -> Result<(), OpError> {
    if caller.via == super::Via::Agent {
        return Err(OpError::Api(crate::error::ApiError::Denied(
            "an agent run does not start, cancel or publish another run; a person does (AG-11)"
                .into(),
        )));
    }
    Ok(())
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_run_create",
            title: "Start Unattended Work",
            description: "Starts an application, dashboard or analysis run, which waits for a person's approval at the end",
            input: create_input_schema,
            output: run_output_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "App",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| {
                if val.get("appName").and_then(Value::as_str).is_none() {
                    return Err(OpError::InvalidInput {
                        path: "/appName".into(),
                        message: "missing field `appName`".into(),
                    });
                }
                Ok(())
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller)?;
                    let request = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = super::serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    let (_, Json(created)) = crate::api::agent_runs::create_run(
                        as_user(caller),
                        State(state.clone()),
                        Path(project.to_owned()),
                        Json(request),
                    )
                    .await?;
                    Ok(serde_json::to_value(created)?)
                })
            },
        },
        Operation {
            name: "jc_run_cancel",
            title: "Cancel Run",
            description: "Stops one of this caller's runs; what it had not finished is not published",
            input: run_id_input_schema,
            output: run_output_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: true,
            },
            kind: "App",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<RunIdInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller)?;
                    let input: RunIdInput = parse_input(val)?;
                    let Json(run) = crate::api::agent_runs::cancel_run(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.id)),
                    )
                    .await?;
                    Ok(serde_json::to_value(run)?)
                })
            },
        },
        Operation {
            name: "jc_run_publish",
            title: "Publish What A Run Built",
            description: "Proposes the application a finished run built; a person approves the change",
            input: run_id_input_schema,
            output: super::change_schema,
            annotations: Annotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "App",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<RunIdInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller)?;
                    let input: RunIdInput = parse_input(val)?;
                    // The route answers the `Change` as a response; the operation answers the
                    // document inside it, as every other propose does.
                    let response = crate::api::agent_runs::publish_run(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.id)),
                    )
                    .await?;
                    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                        .await
                        .map_err(|err| {
                            OpError::Api(crate::error::ApiError::Internal(err.to_string()))
                        })?;
                    Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
                })
            },
        },
    ]
}
