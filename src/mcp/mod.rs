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
//! - `tools/call`: executes an operation via the shared registry and returns structured results.
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
//!   Yellow/Red proposals through MCP return the proposed Change directly (`{changeId, lane, url}`),
//!   as the required approval by a human reviewer serves as the interaction gate.
//!
//! ponytail: per-token rate limiter not yet present in portal (AG-60); skipped per T-0637 instructions.

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;

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
                            "prompts": {}
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
                            "prompts": {}
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
        "resources/list" => {
            let project = project_of(&params, &state, &caller);
            if !may_read(&state, &caller, &project) {
                return json_response(StatusCode::OK, &result(id, json!({ "resources": [] })));
            }
            let mut resources = Vec::new();
            for info in crate::resource::kinds() {
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
    let eff = crate::permissions::for_request(state, &caller.identity, project);
    eff.bootstrap || !eff.grants.is_empty()
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
        [project, "drafts", kind, name] if may_read(state, caller, project) => {
            let draft = crate::ops::drafts::draft_store(state)
                .get(project, kind, name)
                .await
                .ok()
                .flatten()?;
            Some(("application/json", serde_json::to_string(&draft).ok()?))
        }
        [project, plural, name] if may_read(state, caller, project) => {
            let info = crate::resource::by_plural(plural)?;
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

fn json_response(status: StatusCode, body: &Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec()),
        ))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
