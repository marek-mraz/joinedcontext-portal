//! The Portal Configuration MCP server at `/api/v1/mcp` (T-0637, AG-60, CC-45, ADR-N-021).
//!
//! Streamable HTTP over JSON-RPC 2.0, stateless per request.
//!
//! Exposed methods:
//! - `initialize`: negotiates MCP protocol version and returns server capabilities.
//! - `notifications/initialized`: acknowledgement notification (202 Accepted).
//! - `server/discover`: returns server capabilities and the available tool count.
//! - `ping`: health check answering empty result `{}`.
//! - `tools/list`: operation registry filtered by the caller's effective permissions.
//! - `tools/call`: executes an operation via the shared registry and returns structured results;
//!   an operation that runs longer than a request answers a task instead (see [`tasks`]).
//! - `resources/list`, `resources/read`, `prompts/list`, `prompts/get`.
//! - `tasks/get`, `tasks/result`, `tasks/list`, `tasks/cancel`: the Tasks extension (AG-60).
//!
//! Authentication:
//! - Strictly `Authorization: Bearer <jwt>` verified against the realm JWKS.
//! - The token audience (`aud`) must match the Portal client id.
//! - Missing or invalid tokens return 401 with `WWW-Authenticate: Bearer resource_metadata="..."`.
//! - Edge session cookies are deliberately ignored on this route.
//!
//! RFC 9728:
//! - `GET /.well-known/oauth-protected-resource/api/v1/mcp` answers the metadata document.
//!
//! Note on interaction lanes (AG-61..AG-63):
//! - Drafts (AG-61) and verdict gating (AG-62) are supported across operations.
//! - A call whose operation takes the Yellow or the Red lane, or carries `destructiveHint`,
//!   asks the person first (AG-63): the first `tools/call` runs nothing and answers an
//!   elicitation, the client repeats it with the person's answer, and the answer is written to
//!   the project's activity. See [`elicitation`]. The approval of the change it opens is the
//!   second gate, not the first.
//!
//! Limits (AG-60), counted here and not only at the edge, whose bucket is keyed by the raw
//! token string and does not exist for a caller inside the cluster (T-0839):
//! - `MAX_CALLS_PER_MINUTE` per token subject; the call past it answers `429` with `Retry-After`.
//! - `MAX_REQUEST_BYTES` on the request and `MAX_RESPONSE_BYTES` on the answer; either answers
//!   `413` naming what to narrow, so one call cannot spend the Portal's memory on one client.

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;

pub mod elicitation;
pub mod tasks;

/// Calls one bearer subject may make in a minute (AG-60). Generous for a working agent, far
/// under what a loop costs: the edge's 1200 a minute is shared with the whole REST API.
const MAX_CALLS_PER_MINUTE: u32 = 120;
/// The largest request this route reads: a manifest, a draft or a bundle of arguments.
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// The largest answer it writes. A list that does not fit is narrowed by the caller, never
/// streamed out of the Portal's memory in one piece.
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/mcp",
            post(handle_mcp).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
        .route(
            "/.well-known/oauth-protected-resource/api/v1/mcp",
            get(oauth_protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth_protected_resource),
        )
}

/// RFC 9728 OAuth 2.0 Protected Resource Metadata document.
pub async fn oauth_protected_resource(State(state): State<AppState>) -> Response {
    let resource = format!(
        "{}/api/v1/mcp",
        state.config.public_base_url.as_str().trim_end_matches('/')
    );
    let authorization_servers = if let Some(oidc) = state.config.oidc.as_ref() {
        vec![oidc.issuer.as_str().trim_end_matches('/').to_string()]
    } else {
        Vec::new()
    };

    let doc = json!({
        "resource": resource,
        "authorization_servers": authorization_servers,
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["mcp:portal"]
    });

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        Json(doc),
    )
        .into_response()
}

/// JSON-RPC 2.0 dispatcher for the Configuration MCP server.
pub async fn handle_mcp(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let Some(token) = auth_header
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return unauthorized_response(&state);
    };

    let Some(verifier) = state.bearer.as_ref() else {
        return unauthorized_response(&state);
    };

    let session = match verifier.verify(token).await {
        Ok(s) => s,
        Err(_) => return unauthorized_response(&state),
    };

    if state.is_revoked(&session) {
        return unauthorized_response(&state);
    }

    if body.len() > MAX_REQUEST_BYTES {
        return too_large(&format!(
            "the request is {} bytes; this route reads at most {MAX_REQUEST_BYTES}. Send fewer \
             arguments, or put a large manifest in a draft and name it",
            body.len()
        ));
    }

    if !state.mcp_call_allowed(
        &session.identity.subject,
        MAX_CALLS_PER_MINUTE,
        crate::auth::session::now_unix(),
    ) {
        return rate_limited();
    }

    let caller = crate::ops::Caller {
        identity: session.identity,
        via: crate::ops::Via::Mcp,
    };

    let message: Value = match serde_json::from_slice::<Value>(&body) {
        Ok(v) if v.is_object() => v,
        _ => return parse_error(),
    };

    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    if method == "notifications/initialized" {
        return StatusCode::ACCEPTED.into_response();
    }

    let Some(id) = message.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };

    match method {
        "initialize" => {
            let client_version = params.get("protocolVersion").and_then(Value::as_str);
            let protocol_version = match client_version {
                Some(v) if v == "2026-07-28" || v == "2025-06-18" || v == "2025-03-26" => v,
                Some(v) => v,
                None => "2026-07-28",
            };
            json_response(
                StatusCode::OK,
                &result(
                    id,
                    json!({
                        "protocolVersion": protocol_version,
                        "capabilities": {
                            "tools": {},
                            "resources": {},
                            "prompts": {},
                            "tasks": {}
                        },
                        "serverInfo": {
                            "name": "joinedcontext-portal",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }),
                ),
            )
        }
        "server/discover" => {
            let client_version = params.get("protocolVersion").and_then(Value::as_str);
            let protocol_version = client_version.unwrap_or("2026-07-28");
            let count = if let Some(project) = params.get("project").and_then(Value::as_str) {
                crate::ops::listing(&caller, &state, project).len()
            } else {
                crate::ops::registry().len()
            };
            json_response(
                StatusCode::OK,
                &result(
                    id,
                    json!({
                        "protocolVersion": protocol_version,
                        "capabilities": {
                            "tools": {},
                            "resources": {},
                            "prompts": {},
                            "tasks": {}
                        },
                        "serverInfo": {
                            "name": "joinedcontext-portal",
                            "version": env!("CARGO_PKG_VERSION")
                        },
                        "tools": {
                            "count": count
                        }
                    }),
                ),
            )
        }
        "ping" => json_response(StatusCode::OK, &result(id, json!({}))),
        "tools/list" => {
            let project = project_of(&params, &state, &caller);
            let ops = crate::ops::listing(&caller, &state, &project);
            let tools: Vec<Value> = ops
                .into_iter()
                .map(|op| {
                    json!({
                        "name": op.name,
                        "title": op.title,
                        "description": op.description,
                        "inputSchema": mcp_input_schema(op.input_schema),
                        "outputSchema": op.output_schema,
                        "annotations": {
                            "readOnlyHint": op.annotations.read_only_hint,
                            "destructiveHint": op.annotations.destructive_hint,
                            "idempotentHint": op.annotations.idempotent_hint,
                        },
                        // AG-60: a call that leaves the Portal and waits is served as a task.
                        "execution": {
                            "taskSupport": if tasks::is_long(&op.name) { "required" } else { "forbidden" }
                        }
                    })
                })
                .collect();

            json_response(StatusCode::OK, &result(id, json!({ "tools": tools })))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let project = arguments
                .get("project")
                .and_then(Value::as_str)
                .or_else(|| params.get("project").and_then(Value::as_str));

            let Some(project) = project.filter(|p| !p.trim().is_empty()) else {
                return json_response(
                    StatusCode::OK,
                    &result(
                        id,
                        json!({
                            "isError": true,
                            "content": [{
                                "type": "text",
                                "text": "missing required argument: project"
                            }]
                        }),
                    ),
                );
            };

            let Some(op) = crate::ops::find(name) else {
                return json_response(StatusCode::OK, &error(id, -32602, "unknown tool"));
            };

            let mut input = arguments.clone();
            if let Some(map) = input.as_object_mut() {
                map.remove("project");
            }

            // AG-63: a Yellow or Red lane, or a destructive tool, asks the person before it
            // runs. The first call answers the question; the second carries their answer.
            if op.lane != crate::change::Lane::Green || op.annotations.destructive_hint {
                let owner = caller.identity.subject.as_str();
                let digest = elicitation::digest_of(&input);
                match params.get("elicitation") {
                    None => {
                        let elicitation_id =
                            state.mcp_elicitations.ask(owner, op.name, project, &digest);
                        return json_response(
                            StatusCode::OK,
                            &result(
                                id,
                                elicitation::document(
                                    &elicitation_id,
                                    &format!(
                                        "{} in project '{project}' ({} lane). Nothing has run: \
                                         open the Portal, look at what this would change, and \
                                         send this call again with the answer.",
                                        op.title,
                                        lane_word(op.lane)
                                    ),
                                    &confirm_url(&state, project, op.kind, &input),
                                ),
                            ),
                        );
                    }
                    Some(sent) => match state
                        .mcp_elicitations
                        .answer(owner, op.name, project, &digest, sent)
                    {
                        elicitation::Answer::Accepted => {
                            record_answer(&state, project, &caller, op.name, "accepted").await;
                        }
                        elicitation::Answer::Declined => {
                            record_answer(&state, project, &caller, op.name, "declined").await;
                            return json_response(
                                StatusCode::OK,
                                &result(
                                    id,
                                    failure_result(
                                        "the person declined this call; nothing was run (AG-63)",
                                    ),
                                ),
                            );
                        }
                        elicitation::Answer::Unknown => {
                            return json_response(
                                StatusCode::OK,
                                &result(
                                    id,
                                    failure_result(
                                        "that answer belongs to no open question of this call: \
                                         an answer is spent once, expires in ten minutes and is \
                                         bound to these arguments. Call again without \
                                         `elicitation` to ask anew (AG-63)",
                                    ),
                                ),
                            );
                        }
                    },
                }
            }

            // A call that waits on the runner, on Model Tools or on somebody's feed answers a
            // task at once; the client polls it instead of holding the request open (AG-60).
            if tasks::is_long(name) || params.get("task").is_some() {
                let owner = caller.identity.subject.clone();
                let state_for_task = state.clone();
                let project = project.to_owned();
                let op_name = op.name;
                let task = state.mcp_tasks.start(&owner, async move {
                    let op = crate::ops::find(op_name).expect("the operation was found above");
                    match crate::ops::call(op, &caller, &state_for_task, &project, input).await {
                        Ok(output) => call_result(&output),
                        Err(crate::ops::OpError::Conflict(val)) => conflict_result(&val),
                        Err(err) => failure_result(&err.to_string()),
                    }
                });
                return json_response(StatusCode::OK, &result(id, json!({ "task": task })));
            }

            let response_val = match crate::ops::call(op, &caller, &state, project, input).await {
                Ok(output) => result(
                    id,
                    json!({
                        "isError": false,
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string(&output).unwrap_or_else(|_| "null".to_string())
                        }],
                        "structuredContent": output
                    }),
                ),
                Err(crate::ops::OpError::Conflict(val)) => result(
                    id,
                    json!({
                        "isError": true,
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string(&val).unwrap_or_else(|_| "conflict".to_string())
                        }],
                        "structuredContent": val
                    }),
                ),
                Err(err) => result(
                    id,
                    json!({
                        "isError": true,
                        "content": [{
                            "type": "text",
                            "text": err.to_string()
                        }]
                    }),
                ),
            };

            json_response(StatusCode::OK, &response_val)
        }
        "tasks/get" | "tasks/result" | "tasks/cancel" | "tasks/list" => {
            let owner = caller.identity.subject.as_str();
            if method == "tasks/list" {
                return json_response(
                    StatusCode::OK,
                    &result(id, json!({ "tasks": state.mcp_tasks.list(owner) })),
                );
            }
            let Some(task_id) = params.get("taskId").and_then(Value::as_str) else {
                return json_response(StatusCode::OK, &error(id, -32602, "missing taskId"));
            };
            // A task of another caller is answered exactly as one that never existed.
            match method {
                "tasks/get" => match state.mcp_tasks.describe(owner, task_id) {
                    Some(task) => json_response(StatusCode::OK, &result(id, task)),
                    None => json_response(StatusCode::OK, &error(id, -32602, "unknown task")),
                },
                "tasks/cancel" => match state.mcp_tasks.cancel(owner, task_id) {
                    Some(task) => json_response(StatusCode::OK, &result(id, task)),
                    None => json_response(StatusCode::OK, &error(id, -32602, "unknown task")),
                },
                _ => match state.mcp_tasks.result(owner, task_id) {
                    Ok(Some(value)) => json_response(StatusCode::OK, &result(id, value)),
                    Ok(None) => json_response(StatusCode::OK, &error(id, -32602, "unknown task")),
                    Err(said) => json_response(StatusCode::OK, &error(id, -32002, said)),
                },
            }
        }
        "resources/list" => {
            let project = project_of(&params, &state, &caller);
            if !may_read(&state, &caller, &project) {
                return json_response(StatusCode::OK, &result(id, json!({ "resources": [] })));
            }
            let mut resources = Vec::new();
            for info in crate::resource::kinds() {
                if !may_read_kind(&state, &caller, &project, info.kind) {
                    continue;
                }
                let page =
                    state
                        .mirror
                        .list(&project, info.kind, &crate::store::ListOptions::default());
                for item in page.items {
                    resources.push(json!({
                        "uri": format!("jc://{project}/{}/{}", info.plural, item.metadata.name),
                        "name": item.metadata.name,
                        "title": format!("{} {}", info.kind, item.metadata.name),
                        "mimeType": "application/json"
                    }));
                }
            }
            let drafts = crate::ops::drafts::draft_store(&state)
                .list(&project)
                .await
                .unwrap_or_default();
            for draft in drafts {
                resources.push(json!({
                    "uri": format!("jc://{project}/drafts/{}/{}", draft.kind, draft.name),
                    "name": draft.name,
                    "title": format!("Draft {} {}", draft.kind, draft.name),
                    "mimeType": "application/json"
                }));
            }
            for info in jc_core::KINDS {
                resources.push(json!({
                    "uri": format!("jc://schemas/{}", info.kind),
                    "name": info.kind,
                    "title": format!("JSON Schema of {}", info.kind),
                    "mimeType": "application/schema+json"
                }));
            }
            json_response(
                StatusCode::OK,
                &result(id, json!({ "resources": resources })),
            )
        }
        "resources/read" => {
            let uri = params.get("uri").and_then(Value::as_str).unwrap_or("");
            match read_resource(&state, &caller, uri).await {
                Some((mime, text)) => json_response(
                    StatusCode::OK,
                    &result(
                        id,
                        json!({ "contents": [{ "uri": uri, "mimeType": mime, "text": text }] }),
                    ),
                ),
                None => json_response(StatusCode::OK, &error(id, -32002, "resource not found")),
            }
        }
        "prompts/list" => {
            let prompts: Vec<Value> = UNITS
                .iter()
                .map(|(name, description, _)| {
                    json!({
                        "name": name,
                        "title": format!("{} in 60 seconds", capitalise(name)),
                        "description": description,
                        "arguments": [{
                            "name": "project",
                            "description": "Project slug",
                            "required": true
                        }]
                    })
                })
                .collect();
            json_response(StatusCode::OK, &result(id, json!({ "prompts": prompts })))
        }
        "prompts/get" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let Some((_, description, steps)) = UNITS.iter().find(|(n, _, _)| *n == name) else {
                return json_response(StatusCode::OK, &error(id, -32602, "unknown prompt"));
            };
            let project = params
                .get("arguments")
                .and_then(|a| a.get("project"))
                .and_then(Value::as_str)
                .unwrap_or("<project>");
            let text = format!("{description}\nProject: {project}.\n{steps}");
            json_response(
                StatusCode::OK,
                &result(
                    id,
                    json!({
                        "description": description,
                        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }]
                    }),
                ),
            )
        }
        _ => json_response(StatusCode::OK, &error(id, -32601, "method not found")),
    }
}

/// The units as prompts: name, one-line description, the operations in order (AG-63).
const UNITS: &[(&str, &str, &str)] = &[
    (
        "load",
        "Load a data source into a space: check the source, build and test the pipeline, propose both.",
        "1. jc_draft_put a DataSource, jc_datasource_check it until the verdict is ok.\n2. jc_draft_put a Pipeline that maps the sample, jc_pipeline_test it.\n3. jc_datasource_propose and jc_pipeline_propose; the change waits for approval.",
    ),
    (
        "share",
        "Share a space with another organization through an endpoint and its policy.",
        "1. jc_catalog_search for the space and its entity types.\n2. jc_draft_put an Endpoint, jc_manifest_dry_run it.\n3. jc_endpoint_propose; jc_change_list shows the change the approver decides.",
    ),
    (
        "analyse",
        "Compute a KPI over a space and write it back through an endpoint.",
        "1. jc_catalog_search for the source endpoint.\n2. jc_kpi_compute on a sample to see the numbers.\n3. jc_draft_put a KPI Pipeline, jc_pipeline_test it, jc_pipeline_propose.",
    ),
    (
        "model",
        "Infer a LinkML data model from samples and propose it.",
        "1. jc_model_infer over the samples.\n2. jc_draft_put the DataModel, jc_manifest_dry_run it.\n3. jc_model_propose.",
    ),
    (
        "change",
        "Change or remove anything in the project: find it, change its manifest or propose its removal; a person approves.",
        "1. jc_resource_list the kind, jc_resource_get the one to change.\n2. jc_draft_put the changed manifest, jc_manifest_dry_run it until the verdict is ok.\n3. jc_resource_propose the draft, or jc_resource_delete with its name typed back.\n4. jc_change_list shows the change; a person approves or rejects it.",
    ),
];

fn capitalise(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The project named in the params, else the first one the caller may read.
fn project_of(params: &Value, state: &AppState, caller: &crate::ops::Caller) -> String {
    if let Some(project) = params.get("project").and_then(Value::as_str) {
        return project.to_string();
    }
    state
        .mirror
        .namespaces()
        .into_iter()
        .find(|ns| may_read(state, caller, ns))
        .unwrap_or_else(|| "default".to_string())
}

fn may_read(state: &AppState, caller: &crate::ops::Caller, project: &str) -> bool {
    crate::permissions::for_request(state, &caller.identity, project).may_read_project()
}

/// Whether the caller reads this kind here (PF-59): the same rule the REST list and get apply,
/// so a resource the API hides is not handed out over MCP either.
fn may_read_kind(state: &AppState, caller: &crate::ops::Caller, project: &str, kind: &str) -> bool {
    crate::permissions::for_request(state, &caller.identity, project).may_read(kind)
}

/// `jc://schemas/{Kind}`, `jc://{project}/drafts/{Kind}/{name}` or `jc://{project}/{plural}/{name}`.
async fn read_resource(
    state: &AppState,
    caller: &crate::ops::Caller,
    uri: &str,
) -> Option<(&'static str, String)> {
    let rest = uri.strip_prefix("jc://")?;
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        ["schemas", kind] => {
            let schema = jc_core::registry::schema_of(kind)?;
            Some(("application/schema+json", schema.to_string()))
        }
        [project, "drafts", kind, name] if may_read_kind(state, caller, project, kind) => {
            let draft = crate::ops::drafts::draft_store(state)
                .get(project, kind, name)
                .await
                .ok()
                .flatten()?;
            Some(("application/json", serde_json::to_string(&draft).ok()?))
        }
        [project, plural, name] => {
            let info = crate::resource::by_plural(plural)?;
            if !may_read_kind(state, caller, project, info.kind) {
                return None;
            }
            let item = state.mirror.get(project, info.kind, name)?;
            Some(("application/json", serde_json::to_string(&item).ok()?))
        }
        _ => None,
    }
}

/// Ensures the tool's input schema exposes `project` as a required parameter for MCP callers.
fn mcp_input_schema(mut schema: Value) -> Value {
    if !schema.is_object() {
        schema = json!({ "type": "object" });
    }
    if let Some(obj) = schema.as_object_mut() {
        if !obj.contains_key("type") {
            obj.insert("type".to_string(), json!("object"));
        }
        let props = obj.entry("properties").or_insert_with(|| json!({}));
        if let Some(props_map) = props.as_object_mut() {
            if !props_map.contains_key("project") {
                props_map.insert(
                    "project".to_string(),
                    json!({
                        "type": "string",
                        "description": "Project slug"
                    }),
                );
            }
        }
        let req = obj.entry("required").or_insert_with(|| json!([]));
        if let Some(req_arr) = req.as_array_mut() {
            if !req_arr.iter().any(|v| v.as_str() == Some("project")) {
                req_arr.push(json!("project"));
            }
        }
    }
    schema
}

fn unauthorized_response(state: &AppState) -> Response {
    let resource_metadata = format!(
        "{}/.well-known/oauth-protected-resource/api/v1/mcp",
        state.config.public_base_url.as_str().trim_end_matches('/')
    );
    let mut resp = ApiError::Unauthorized.into_response();
    if let Ok(header_val) = format!("Bearer resource_metadata=\"{resource_metadata}\"").parse() {
        resp.headers_mut()
            .insert(header::WWW_AUTHENTICATE, header_val);
    }
    resp
}

fn result(id: Value, value: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": value })
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn parse_error() -> Response {
    let body = json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "error": { "code": -32700, "message": "parse error" }
    });
    json_response(StatusCode::OK, &body)
}

/// The lane, as the sentence a person reads names it.
fn lane_word(lane: crate::change::Lane) -> &'static str {
    match lane {
        crate::change::Lane::Green => "green",
        crate::change::Lane::Yellow => "yellow",
        crate::change::Lane::Red => "red",
    }
}

/// Where the person looks at what the call would do: the project's page for that kind.
///
/// An operation of one kind names it; `jc_resource_*` takes any kind, so the arguments name it
/// and the page is the one holding the resource the call is about.
fn confirm_url(state: &AppState, project: &str, kind: &str, input: &Value) -> String {
    let base = state.config.public_base_url.as_str().trim_end_matches('/');
    let named = input
        .get("kind")
        .and_then(Value::as_str)
        .or_else(|| input.pointer("/manifest/kind").and_then(Value::as_str))
        .or_else(|| input.pointer("/draft/kind").and_then(Value::as_str));
    let kind = match kind {
        "*" => named.unwrap_or(kind),
        named_by_operation => named_by_operation,
    };
    match crate::resource::by_kind(kind) {
        Some(info) => format!("{base}/projects/{project}/{}", info.plural),
        None => format!("{base}/projects/{project}"),
    }
}

/// The person's answer, on the project's activity: who allowed what, and when (AG-56, AG-63).
async fn record_answer(
    state: &AppState,
    project: &str,
    caller: &crate::ops::Caller,
    operation: &str,
    decision: &str,
) {
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "mcp.tool".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} {decision} {operation} asked through MCP",
            caller.identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({
            "operation": operation,
            "decision": decision,
            "subject": caller.identity.subject,
        }),
    };
    let _ = state.activity.append(&[event]).await;
}

/// One operation's answer as a `tools/call` result.
fn call_result(output: &Value) -> Value {
    json!({
        "isError": false,
        "content": [{
            "type": "text",
            "text": serde_json::to_string(output).unwrap_or_else(|_| "null".to_string())
        }],
        "structuredContent": output
    })
}

/// A conflict the caller resolves: the document says which and what to do.
fn conflict_result(val: &Value) -> Value {
    json!({
        "isError": true,
        "content": [{
            "type": "text",
            "text": serde_json::to_string(val).unwrap_or_else(|_| "conflict".to_string())
        }],
        "structuredContent": val
    })
}

/// A refusal, in the agent's own channel for it.
fn failure_result(said: &str) -> Value {
    json!({
        "isError": true,
        "content": [{ "type": "text", "text": said }]
    })
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    if bytes.len() > MAX_RESPONSE_BYTES {
        return too_large(&format!(
            "the answer is {} bytes; this route writes at most {MAX_RESPONSE_BYTES}. Ask for \
             fewer items, one project, or one resource by name",
            bytes.len()
        ));
    }
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// A request or an answer past the byte limit (AG-60): the status, and what to narrow.
fn too_large(detail: &str) -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        [(header::CONTENT_TYPE, "application/json")],
        Json(json!({
            "jsonrpc": "2.0",
            "error": { "code": -32001, "message": detail }
        })),
    )
        .into_response()
}

/// One subject past its calls a minute (AG-60): the next minute is when it may call again.
fn rate_limited() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::RETRY_AFTER, "60"),
        ],
        Json(json!({
            "jsonrpc": "2.0",
            "error": {
                "code": -32000,
                "message": format!(
                    "this token has made {MAX_CALLS_PER_MINUTE} calls in a minute, the limit of \
                     this route; the next minute opens a new budget (AG-60)"
                )
            }
        })),
    )
        .into_response()
}
