//! One operation registry behind the UI, the API, the assistant, and MCP (AG-59, ADR-N-021).
//!
//! Every action the Portal offers is an operation: a name, input/output schemas,
//! behaviour annotations, required roles/verbs, and an interaction lane (CC-70).
//! REST and MCP surfaces act as thin adapters over these same functions (CC-48).
//!
//! Note on drafts (AG-61), verdict gating (AG-62) and elicitation (AG-63): these are
//! later increments (T-0638/T-0639). Proposal operations currently return the planned Change
//! envelope directly since human review on the merge request serves as the gate.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

pub mod admin;
pub mod drafts;
pub mod feed_shape;
pub mod previews;
pub mod resources;
pub mod runs;
pub mod space_complete;
pub mod sync_sources;
pub mod verdict;
pub mod views;
pub mod workspaces;

pub use drafts::*;
pub use verdict::*;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use jc_core::kinds::Verb;
use jcctl::pipeline_test::Sample;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::agents::kpi;
use crate::agents::share;
use crate::api::assistant;
use crate::api::changes;
use crate::api::dry_run;
use crate::api::mutate;
use crate::api::pipeline_test;
use crate::auth::session::Identity;
use crate::change::{Lane, Operation as ChangeOp};
use crate::error::ApiError;
use crate::state::AppState;
use crate::tools::model_tools;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Session,
    Bearer,
    Mcp,
    /// An assistant or application run acting for the person who started it (AG-70).
    Agent,
}

impl Via {
    /// Who last touched a draft, as the draft records it (AG-61).
    pub fn touched_kind(self) -> &'static str {
        match self {
            Via::Session => "person",
            Via::Bearer => "api-key",
            Via::Mcp => "mcp",
            Via::Agent => "run",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Caller {
    pub identity: Identity,
    pub via: Via,
    /// What the run's `AgentProfile` grants, when this call belongs to an agent run (AG-70).
    ///
    /// `None` is a person at a keyboard or a program with their own token: their bindings alone
    /// decide. A profile is the second half and only ever narrows — an operation the person may
    /// not run stays refused whether the profile names it or not.
    pub access: Option<crate::agents::access::Access>,
}

impl Caller {
    pub fn new(identity: Identity, via: Via) -> Self {
        Self {
            identity,
            via,
            access: None,
        }
    }

    /// A call made inside an agent run: the person who started it, narrowed by its profile.
    pub fn for_run(identity: Identity, access: crate::agents::access::Access) -> Self {
        Self {
            identity,
            via: Via::Agent,
            access: Some(access),
        }
    }

    /// The profile's half of a call, where a caller has one (AG-70).
    pub fn grants(&self, op: &Operation) -> Result<(), OpError> {
        match &self.access {
            Some(access) if !access.names(op) => Err(OpError::Api(ApiError::Denied(
                crate::agents::access::refusal(op.name),
            ))),
            _ => Ok(()),
        }
    }

    /// Whether this caller may run the operation at all — asked before anything is asked of a
    /// person (AG-63).
    ///
    /// A refusal that arrives after the confirmation has the person answer a question whose
    /// answer changes nothing, and it teaches an agent that approving is something it does and
    /// then fails at. The profile's half (AG-70) and the refusal no profile can lift (AG-11)
    /// are both knowable from the caller and the operation alone, so they are decided here.
    pub fn may_run(&self, op: &Operation) -> Result<(), OpError> {
        // AG-11 before AG-70: a profile that names an approval is an author's mistake, and being
        // told the profile does not grant what it plainly lists explains nothing. The true reason
        // is that no profile can grant it.
        if op.kind == "Change" && op.verb.is_some() {
            resources::refuse_agent_decision(self)?;
        }
        if op.name == "jc_workspace_propose" {
            workspaces::refuse_agent_bring_back(self)?;
        }
        self.grants(op)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationAnnotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
}

pub type Annotations = OperationAnnotations;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationSummary {
    pub name: String,
    pub title: String,
    pub description: String,
    #[schema(value_type = Object)]
    pub input_schema: Value,
    #[schema(value_type = Object)]
    pub output_schema: Value,
    pub annotations: OperationAnnotations,
    pub lane: Lane,
}

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("forbidden: missing role {0}")]
    Forbidden(String),
    #[error("invalid input at {path}: {message}")]
    InvalidInput { path: String, message: String },
    #[error("conflict: {0}")]
    Conflict(Value),
    #[error(transparent)]
    Api(#[from] ApiError),
}

impl From<serde_json::Error> for OpError {
    fn from(err: serde_json::Error) -> Self {
        OpError::Api(ApiError::Internal(err.to_string()))
    }
}

impl IntoResponse for OpError {
    fn into_response(self) -> Response {
        match self {
            OpError::Forbidden(role) => (
                StatusCode::FORBIDDEN,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                Json(json!({
                    "error": "forbidden",
                    "role": role,
                })),
            )
                .into_response(),
            OpError::InvalidInput { path, message } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                Json(json!({
                    "error": "invalid_input",
                    "path": path,
                    "message": message,
                })),
            )
                .into_response(),
            OpError::Conflict(val) => (
                StatusCode::CONFLICT,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                Json(val),
            )
                .into_response(),
            OpError::Api(err) => err.into_response(),
        }
    }
}

impl From<OpError> for ApiError {
    fn from(err: OpError) -> Self {
        match err {
            OpError::Forbidden(msg) => ApiError::Denied(msg),
            OpError::InvalidInput { path, message } => {
                ApiError::BadRequest(format!("{path}: {message}"))
            }
            // The verdict gate's document travels whole, so a REST door answers what the
            // operation answers (T-0956).
            OpError::Conflict(val) if val.get("error") == Some(&json!("verdict_required")) => {
                ApiError::VerdictRequired(val)
            }
            OpError::Conflict(val) => ApiError::Conflict(
                val.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("conflict")
                    .to_owned(),
            ),
            OpError::Api(api) => api,
        }
    }
}

pub struct Operation {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input: fn() -> Value,
    pub output: fn() -> Value,
    pub annotations: OperationAnnotations,
    pub kind: &'static str,
    pub verb: Option<Verb>,
    pub lane: Lane,
    pub validate: fn(&Value) -> Result<(), OpError>,
    pub run: RunFn,
}

/// One operation's body: the caller, the state, the project and the validated input.
pub type RunFn =
    for<'a> fn(&'a Caller, &'a AppState, &'a str, Value) -> BoxFuture<'a, Result<Value, OpError>>;

pub(crate) fn serde_error_path_and_message(err: &serde_json::Error) -> (String, String) {
    let msg = err.to_string();
    if let Some(rest) = msg.strip_prefix("unknown field `") {
        if let Some((field, _)) = rest.split_once('`') {
            return (format!("/{field}"), msg);
        }
    }
    if let Some(rest) = msg.strip_prefix("missing field `") {
        if let Some((field, _)) = rest.split_once('`') {
            return (format!("/{field}"), msg);
        }
    }
    ("/".to_string(), msg)
}

static REGISTRY: OnceLock<Vec<Operation>> = OnceLock::new();

pub fn registry() -> &'static [Operation] {
    REGISTRY.get_or_init(init_registry)
}

pub fn find(name: &str) -> Option<&'static Operation> {
    registry().iter().find(|op| op.name == name)
}

pub async fn call(
    op: &Operation,
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: Value,
) -> Result<Value, OpError> {
    caller.may_run(op)?;
    permitted(op, &caller.identity, state, project)?;
    (op.validate)(&input)?;
    (op.run)(caller, state, project, input).await
}

/// The person's half of a call (PF-50), as the REST route of the same action decides it: the
/// operation's verb on its kind; for a decision on a change, the verb on any kind, the change's own
/// kind being checked when the change is read; for a resource operation, the check its route
/// function makes (AG-77); for any other verbless operation, a grant in the project.
pub fn permitted(
    op: &Operation,
    identity: &crate::auth::session::Identity,
    state: &AppState,
    project: &str,
) -> Result<(), OpError> {
    let effective = crate::permissions::for_request(state, identity, project);
    match (op.kind, op.verb) {
        ("Change", Some(_)) => crate::api::changes::may_approve_anything(state, identity, project)?,
        (kind, Some(verb)) => effective.check(kind, verb, None)?,
        (_, None) if resources::CHECKED_BY_THE_ROUTE.contains(&op.name) => {}
        // A read of a project the caller may not read is the answer of a project that is not
        // there (PF-59, R20); a write keeps its 403, which names what is missing (PF-50).
        (_, None) if !effective.may_read_project() && op.annotations.read_only_hint => {
            return Err(OpError::Api(ApiError::NotFound(format!(
                "project '{project}' not found"
            ))));
        }
        (_, None) if !effective.may_read_project() => {
            return Err(OpError::Api(ApiError::Denied(format!(
                "no role grants access in project {project} (PF-50)"
            ))));
        }
        (_, None) => {}
    }
    Ok(())
}

/// The operations this caller may run in the project: the ones [`permitted`] lets through.
pub fn listing(caller: &Caller, state: &AppState, project: &str) -> Vec<OperationSummary> {
    registry()
        .iter()
        .filter(|op| caller.may_run(op).is_ok())
        .filter(|op| permitted(op, &caller.identity, state, project).is_ok())
        .map(|op| OperationSummary {
            name: op.name.to_string(),
            title: op.title.to_string(),
            description: op.description.to_string(),
            input_schema: (op.input)(),
            output_schema: (op.output)(),
            annotations: op.annotations,
            lane: op.lane,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Input structures with serde(deny_unknown_fields)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSearchInput {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftRef {
    pub kind: String,
    pub name: String,
}

fn draft_error(err: DraftError) -> OpError {
    match err {
        DraftError::Conflict { current } => OpError::Conflict(json!({
            "type": "https://joinedcontext.com/problems/draft-conflict",
            "error": "draft_conflict",
            "current": current,
        })),
        DraftError::Secret(path) => OpError::Api(ApiError::BadRequest(format!(
            "literal secret in field '{path}' is forbidden; use secretRef instead (MF-24)"
        ))),
        DraftError::NotFound {
            project,
            kind,
            name,
        } => OpError::Api(ApiError::NotFound(format!(
            "draft '{kind}/{name}' not found in project '{project}'"
        ))),
        DraftError::Db(msg) => OpError::Api(ApiError::Internal(msg)),
    }
}

fn parse_input<T: serde::de::DeserializeOwned>(val: Value) -> Result<T, OpError> {
    serde_json::from_value(val).map_err(|e| {
        let (path, message) = serde_error_path_and_message(&e);
        OpError::InvalidInput { path, message }
    })
}

/// The verdict of a DataSource check: the dry run's validity and, for an `http` source, the
/// probe of its feed (MF-39).
pub(crate) fn datasource_verdict(out: &Value, manifest: &Value) -> Verdict {
    let ok = out.get("valid").and_then(Value::as_bool).unwrap_or(false);
    let mut findings = Vec::new();
    if !ok {
        findings.push(Finding {
            level: Level::Error,
            path: String::new(),
            message: "the manifest did not pass the dry run".into(),
        });
    }
    if let Some(skipped) = out.pointer("/probe/skipped").and_then(Value::as_str) {
        findings.push(Finding {
            level: Level::Info,
            path: "spec.http.url".into(),
            message: skipped.to_owned(),
        });
    }
    Verdict::new(ok, findings, Some(out.clone()), manifest)
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndpointProposeInput {
    #[serde(default)]
    pub context_space: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
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
    pub rate_limits: Option<share::RateLimits>,
    #[serde(default)]
    pub manifest: Option<Value>,
    #[serde(default)]
    pub draft: Option<DraftRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KpiComputeInput {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(rename = "type")]
    pub entity_type: String,
    #[serde(default)]
    pub attribute: String,
    pub agg: kpi::Agg,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub rows: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestInput {
    #[serde(default)]
    pub manifest: Option<Value>,
    #[serde(default)]
    pub draft: Option<DraftRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineTestInput {
    #[serde(default)]
    pub pipeline: Option<Value>,
    pub sample: Sample,
    #[serde(default)]
    pub draft: Option<DraftRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftPutInput {
    pub kind: String,
    pub name: String,
    pub manifest: Value,
    #[serde(default)]
    pub expected_version: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DraftGetInput {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DraftDropInput {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeApproveInput {
    pub id: String,
    #[serde(default)]
    pub confirm: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInferInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub sample: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

fn catalog_search_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "q": { "type": "string", "description": "Search keyword query" },
            "query": { "type": "string", "description": "Alternative query parameter" },
            "scope": { "type": "string", "description": "Narrow to ContextSpace, Endpoint, or DataModel" }
        },
        "additionalProperties": false
    })
}

fn catalog_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "q": { "type": "string" },
            "items": { "type": "array", "items": { "type": "object" } }
        }
    })
}

fn endpoint_propose_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "contextSpace": { "type": "string" },
            "name": { "type": "string" },
            "title": { "type": "string" },
            "audience": { "type": "string" },
            "allowedProjects": { "type": "array", "items": { "type": "string" } },
            "representations": { "type": "array", "items": { "type": "string" } },
            "hiddenAttributes": { "type": "array", "items": { "type": "string" } },
            "entityTypes": { "type": "array", "items": { "type": "string" } },
            "rateLimits": { "type": "object" },
            "manifest": { "type": "object" },
            "draft": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["kind", "name"],
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

fn kpi_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "title": { "type": "string" },
            "type": { "type": "string" },
            "attribute": { "type": "string" },
            "agg": { "type": "string", "enum": ["avg", "sum", "count", "min", "max"] },
            "unit": { "type": "string" },
            "q": { "type": "string" },
            "rows": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["name", "type", "agg"],
        "additionalProperties": false
    })
}

fn manifest_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "manifest": { "type": "object" },
            "draft": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["kind", "name"],
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

fn draft_put_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string" },
            "name": { "type": "string" },
            "manifest": { "type": "object" },
            "expectedVersion": { "type": "integer" }
        },
        "required": ["kind", "name", "manifest"],
        "additionalProperties": false
    })
}

fn draft_get_input_schema() -> Value {
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

/// The verdict a check leaves on a draft, written out for the clients that read it (AG-62).
fn verdict_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "level": { "type": "string", "enum": ["error", "warning", "info"] },
                        "path": { "type": "string" },
                        "message": { "type": "string" }
                    }
                }
            },
            "trace": { "type": "object" },
            "checkedAt": { "type": "string", "format": "date-time" },
            "inputDigest": { "type": "string", "description": "`sha256:` and the digest of the manifest checked" }
        },
        "required": ["ok", "findings", "checkedAt", "inputDigest"]
    })
}

/// One draft as the operations answer it (T-0838: no `$ref` a client cannot resolve).
fn draft_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": { "type": "string" },
            "kind": { "type": "string" },
            "name": { "type": "string" },
            "manifest": { "type": "object" },
            "verdict": verdict_schema(),
            "touchedBy": { "type": "string" },
            "touchedKind": { "type": "string", "description": "person, assistant, mcp, api-key or agent" },
            "version": { "type": "integer" },
            "updatedAt": { "type": "string", "format": "date-time" }
        },
        "required": ["project", "kind", "name", "manifest", "version"]
    })
}

fn draft_list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "description": "A line per draft; the manifest is read with jc_draft_get (T-2248)",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string" },
                        "name": { "type": "string" },
                        "workspace": { "type": "string" },
                        "touchedBy": { "type": "string" },
                        "touchedKind": { "type": "string" },
                        "version": { "type": "integer" },
                        "updatedAt": { "type": "string", "format": "date-time" },
                        "verdict": {
                            "type": "object",
                            "properties": {
                                "ok": { "type": "boolean" },
                                "findings": { "type": "integer" },
                                "checkedAt": { "type": "string", "format": "date-time" }
                            }
                        }
                    }
                }
            }
        }
    })
}

fn draft_drop_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "dropped": { "type": "boolean" }
        }
    })
}

fn pipeline_test_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pipeline": { "type": "object" },
            "sample": {
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "url": { "type": "string" },
                    "format": { "type": "string", "enum": ["csv", "json", "text"] }
                }
            },
            "draft": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string" },
                    "name": { "type": "string" }
                },
                "required": ["kind", "name"],
                "additionalProperties": false
            }
        },
        "required": ["sample"],
        "additionalProperties": false
    })
}

pub(super) fn empty_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn change_approve_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "confirm": { "type": "string" }
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn model_infer_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "content": { "type": "string", "description": "Base64 encoded sample content" },
            "sample": { "type": "string", "description": "Text sample content" },
            "format": { "type": "string", "enum": ["csv", "xlsx", "json", "pdf"] }
        },
        "additionalProperties": false
    })
}

/// What a check answers: whether the manifest is valid, the lane it would take, the fields it
/// would change, what one fetch of a source returned, and the verdict filed on the draft.
fn dry_run_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "valid": { "type": "boolean" },
            "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
            "plan": {
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "object",
                        "properties": {
                            "create": { "type": "integer" },
                            "update": { "type": "integer" },
                            "delete": { "type": "integer" }
                        }
                    },
                    "fields": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string" },
                                "from": {},
                                "to": {}
                            }
                        }
                    }
                }
            },
            "probe": {
                "type": "object",
                "description": "One fetch of an http DataSource (MF-39); absent for every other kind",
                "properties": {
                    "records": { "type": "integer" },
                    "bytes": { "type": "integer" },
                    "sample": {},
                    "skipped": { "type": "string" }
                }
            },
            "verdict": verdict_schema()
        },
        "required": ["valid", "lane", "plan"]
    })
}

fn pipeline_test_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "input": { "type": "object" },
            "mapping": { "type": "array" },
            "validation": { "type": "array" },
            "errors": { "type": "array" },
            "verdict": { "type": "object" }
        }
    })
}

/// The `Change` resource as an MCP client reads it (T-0838).
///
/// Written out rather than derived from the OpenAPI document: utoipa's schema points at
/// `#/components/schemas/…`, a pointer no MCP client resolves, so a tool that published it
/// described nothing (AG-60).
fn change_document_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "apiVersion": { "type": "string" },
            "kind": { "type": "string", "enum": ["Change"] },
            "metadata": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "`chg-` and eight hex characters; what `jc_change_approve` takes as `id`" },
                    "namespace": { "type": "string" }
                },
                "required": ["name", "namespace"]
            },
            "status": {
                "type": "object",
                "properties": {
                    "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
                    "phase": { "type": "string", "enum": ["PendingApproval", "Deploying", "Merged", "Applied", "Rejected"] },
                    "mergeRequest": { "type": "string" },
                    "plan": {
                        "type": "object",
                        "properties": {
                            "create": { "type": "integer" },
                            "update": { "type": "integer" },
                            "delete": { "type": "integer" }
                        }
                    }
                },
                "required": ["lane", "phase", "plan"]
            }
        },
        "required": ["apiVersion", "kind", "metadata", "status"]
    })
}

/// What every propose, the resource delete and a rejection answer: the id and the lane beside
/// the `Change` itself, which is what the caller gets, not the bare resource (T-0838).
fn change_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "changeId": { "type": "string", "description": "The change's name; `jc_change_approve` takes it as `id`" },
            "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
            "url": { "type": "string", "description": "The merge request to review, when the forge has one" },
            "change": change_document_schema(),
            "warning": { "type": "string", "description": "Present when the draft was proposed without a fresh green verdict" }
        },
        "required": ["changeId", "lane", "change"]
    })
}

/// What `jc_change_list` answers: the list envelope, not one proposal (T-0838).
fn change_list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "apiVersion": { "type": "string" },
            "kind": { "type": "string", "enum": ["ChangeList"] },
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "title": { "type": "string" },
                        "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
                        "phase": { "type": "string" },
                        "mergeRequest": { "type": "string" },
                        "proposedBy": { "type": "string" }
                    }
                }
            }
        },
        "required": ["apiVersion", "kind", "items"]
    })
}

/// The `Change` the approval answers: the resource itself, with no wrapper around it.
fn approved_change_schema() -> Value {
    change_document_schema()
}

/// What `jc_endpoint_propose` answers: a Change when it was given a manifest or a draft, and
/// the rendered Endpoint with its draft policies when it was given the parameters to share
/// data (T-0838). One tool, two answers, both published.
fn proposal_output_schema() -> Value {
    json!({
        "oneOf": [
            change_schema(),
            {
                "type": "object",
                "description": "The rendered proposal a form fills itself from; nothing is written yet",
                "properties": {
                    "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
                    "slug": { "type": "string" },
                    "endpoint": { "type": "object" },
                    "policies": { "type": "array", "items": { "type": "object" } },
                    "prefill": { "type": "object" }
                },
                "required": ["endpoint"]
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// Registry Initialisation
// ---------------------------------------------------------------------------

async fn mutate_manifest(
    caller: &Caller,
    state: &AppState,
    project: &str,
    plural: &'static str,
    manifest: Value,
    dry_run: bool,
    gated: bool,
) -> Result<Value, OpError> {
    let kind = manifest
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let name = manifest
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let op = match name
        .as_deref()
        .and_then(|n| state.mirror.get(project, kind, n))
    {
        Some(_) => ChangeOp::Update,
        None => ChangeOp::Create,
    };

    let outcome = if gated && !dry_run {
        mutate::propose_gated(
            &caller.identity,
            state,
            project,
            plural,
            name.as_deref(),
            op,
            manifest,
        )
        .await?
    } else {
        mutate::propose_with_identity(
            &caller.identity,
            state,
            project,
            plural,
            name.as_deref(),
            op,
            dry_run,
            manifest,
        )
        .await?
    };

    Ok(outcome.into_value())
}

async fn resolve_manifest_input(
    state: &AppState,
    project: &str,
    input: &ManifestInput,
) -> Result<(Value, Option<DraftRef>), OpError> {
    if let Some(d) = &input.draft {
        if let Some(m) = &input.manifest {
            return Ok((m.clone(), Some(d.clone())));
        }
        let draft = draft_store(state)
            .get(project, &d.kind, &d.name)
            .await
            .map_err(|e| OpError::Api(ApiError::Internal(e.to_string())))?
            .ok_or_else(|| {
                ApiError::NotFound(format!(
                    "draft '{}/{}' not found in project '{project}'",
                    d.kind, d.name
                ))
            })?;
        return Ok((draft.manifest, Some(d.clone())));
    }
    if let Some(m) = &input.manifest {
        // Checked under its own kind and name, so its proposal finds the verdict (T-0956).
        return Ok((m.clone(), own_draft(m)));
    }
    Err(OpError::InvalidInput {
        path: "/manifest".into(),
        message: "either manifest or draft is required".into(),
    })
}

/// Records a check's verdict on the draft it names (AG-61, AG-62). A form saves its draft after
/// a debounce, so a check can name a draft that does not exist yet: the check then creates it
/// from the manifest it judged, for a caller who may propose the kind, and a Check followed at
/// once by Propose finds the draft and its verdict instead of a 404.
async fn record_verdict(
    caller: &Caller,
    state: &AppState,
    project: &str,
    draft: &DraftRef,
    manifest: &Value,
    verdict: &Verdict,
) {
    let store = draft_store(state);
    if !matches!(
        store
            .set_verdict(project, &draft.kind, &draft.name, verdict.clone())
            .await,
        Err(DraftError::NotFound { .. })
    ) {
        return;
    }
    let may_propose = crate::permissions::for_request(state, &caller.identity, project)
        .check(&draft.kind, Verb::Propose, None)
        .is_ok();
    if !may_propose {
        return;
    }
    let created = store
        .put(
            project,
            &draft.kind,
            &draft.name,
            manifest.clone(),
            Some(0),
            &caller.identity.username,
            caller.via.touched_kind(),
        )
        .await;
    if created.is_ok() {
        let _ = store
            .set_verdict(project, &draft.kind, &draft.name, verdict.clone())
            .await;
    }
}

/// The check an operation's gate names when it refuses a draft nobody has checked (PF-57).
///
/// One table, read by the operation itself and by the MCP door before it offers an
/// elicitation, so both doors name the same check (ADR-N-021).
pub fn check_operation_for(op_name: &str) -> &'static str {
    match op_name {
        "jc_datasource_propose" => "jc_datasource_check",
        "jc_pipeline_propose" => "jc_pipeline_test",
        _ => "jc_manifest_dry_run",
    }
}

/// The check that judges a manifest of `kind`, as the gate names it.
pub fn check_for_kind(kind: &str) -> &'static str {
    match kind {
        "DataSource" => "jc_datasource_check",
        "Pipeline" => "jc_pipeline_test",
        _ => "jc_manifest_dry_run",
    }
}

/// The draft a manifest proposed without one is checked under: its own kind and name.
fn own_draft(manifest: &Value) -> Option<DraftRef> {
    Some(DraftRef {
        kind: manifest.get("kind")?.as_str()?.to_owned(),
        name: manifest.pointer("/metadata/name")?.as_str()?.to_owned(),
    })
}

/// PF-57 for a manifest proposed without a draft, whatever door it comes through (owner decision
/// T-0956): the verdict its own check recorded under its kind and name, green and fresh for this
/// exact manifest. `Ok(true)` is a lax installation letting an unchecked one through.
pub async fn verdict_for_manifest(
    state: &AppState,
    project: &str,
    manifest: &Value,
) -> Result<bool, OpError> {
    let Some(own) = own_draft(manifest) else {
        // No kind or no name: the proposal refuses the shape itself, naming the field.
        return Ok(false);
    };
    let recorded = draft_store(state)
        .get(project, &own.kind, &own.name)
        .await
        .map_err(|e| OpError::Api(ApiError::Internal(e.to_string())))?
        .and_then(|draft| draft.verdict);
    verdict_gate(
        state,
        recorded.as_ref(),
        manifest,
        check_for_kind(&own.kind),
        "manifest",
    )
}

/// After a manifest proposed without a draft became a Change, the draft its check created goes,
/// but only while it still holds that manifest: a person's draft of the same resource with other
/// content stays theirs.
pub async fn forget_check(state: &AppState, project: &str, manifest: &Value) {
    let Some(own) = own_draft(manifest) else {
        return;
    };
    let store = draft_store(state);
    if let Ok(Some(draft)) = store.get(project, &own.kind, &own.name).await {
        if verdict::digest_of(&draft.manifest) == verdict::digest_of(manifest) {
            let _ = store.drop(project, &own.kind, &own.name).await;
        }
    }
}

/// Records the verdict of a check of a manifest that names no draft, under its own kind and
/// name, so the proposal of the same manifest finds it (T-0956).
pub async fn record_check(
    caller: &Caller,
    state: &AppState,
    project: &str,
    manifest: &Value,
    verdict: &Verdict,
) {
    if let Some(own) = own_draft(manifest) {
        record_verdict(caller, state, project, &own, manifest, verdict).await;
    }
}

/// A bundle has no kind and name of its own, so its check is held under this kind and the
/// caller's name: one checked import per person and project (T-1460).
const IMPORT_CHECK_KIND: &str = "ImportBundle";

/// Records the dry run of a bundle as its check (PF-57 on the import door, T-1460). A plan the
/// import could draw is green; one it could not draw was refused before this with its reason.
pub async fn record_import_check(
    state: &AppState,
    identity: &Identity,
    project: &str,
    subject: &Value,
) {
    let store = draft_store(state);
    let who = &identity.username;
    if store
        .put(
            project,
            IMPORT_CHECK_KIND,
            who,
            subject.clone(),
            None,
            who,
            "import",
        )
        .await
        .is_ok()
    {
        let _ = store
            .set_verdict(
                project,
                IMPORT_CHECK_KIND,
                who,
                Verdict::green(subject, None),
            )
            .await;
    }
}

/// PF-57 for an import: the caller's recorded check of this bundle, green and fresh for the
/// files about to be written. `Ok(true)` is a lax installation letting an unchecked one through.
pub async fn verdict_for_import(
    state: &AppState,
    identity: &Identity,
    project: &str,
    subject: &Value,
) -> Result<bool, OpError> {
    let recorded = draft_store(state)
        .get(project, IMPORT_CHECK_KIND, &identity.username)
        .await
        .map_err(|e| OpError::Api(ApiError::Internal(e.to_string())))?
        .and_then(|draft| draft.verdict);
    verdict_gate(
        state,
        recorded.as_ref(),
        subject,
        "jc_project_import",
        "bundle",
    )
}

/// After an import became a Change, its check goes, so the next import is checked again.
pub async fn forget_import_check(state: &AppState, identity: &Identity, project: &str) {
    let _ = draft_store(state)
        .drop(project, IMPORT_CHECK_KIND, &identity.username)
        .await;
}

/// The refusal an operation's verdict gate already holds for these arguments, or `None` when
/// it lets them through (PF-57, AG-62).
///
/// Asked by a door that answers something else before the operation runs — the MCP door
/// offers an elicitation — so the refusal it carries is the route's own, word for word, and
/// a client reads `verdict_required` wherever it knocks (ADR-N-021, T-0947).
pub async fn verdict_refusal(
    state: &AppState,
    op: &Operation,
    project: &str,
    input: &Value,
) -> Option<Value> {
    if op.verb != Some(Verb::Propose) {
        return None;
    }
    let draft_ref = input.get("draft")?;
    let kind = draft_ref.get("kind").and_then(Value::as_str)?;
    let name = draft_ref.get("name").and_then(Value::as_str)?;
    let draft = draft_store(state).get(project, kind, name).await.ok()??;
    match apply_verdict_gate(state, &draft, check_operation_for(op.name)) {
        Err(OpError::Conflict(body)) => Some(body),
        _ => None,
    }
}

fn apply_verdict_gate(state: &AppState, draft: &Draft, check_op: &str) -> Result<bool, OpError> {
    verdict_gate(
        state,
        draft.verdict.as_ref(),
        &draft.manifest,
        check_op,
        "draft",
    )
}

/// The gate itself, for a verdict and the manifest it must be fresh for: `Ok(true)` lets an
/// unchecked manifest through with a warning (lax), `Ok(false)` finds nothing to say.
fn verdict_gate(
    state: &AppState,
    verdict: Option<&Verdict>,
    manifest: &Value,
    check_op: &str,
    subject: &str,
) -> Result<bool, OpError> {
    let mode = verdict::get_validation_mode(state);
    let reason = match verdict {
        None => Some("verdict_absent"),
        Some(v) if !v.ok => Some("verdict_failed"),
        Some(v) if !v.is_fresh_for(manifest) => Some("stale"),
        _ => None,
    };

    if let Some(reason) = reason {
        if mode == verdict::Validation::Strict {
            let detail = match reason {
                "verdict_absent" => {
                    format!("The {subject} has not been checked; check it, then propose it.")
                }
                "verdict_failed" => {
                    format!(
                        "The {subject}'s check found problems; resolve them and check it again."
                    )
                }
                _ => format!(
                    "The {subject} changed since its check; check it again, then propose it."
                ),
            };
            return Err(OpError::Conflict(json!({
                "error": "verdict_required",
                "check": check_op,
                "reason": reason,
                "detail": detail,
            })));
        } else {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn propose_with_optional_draft(
    caller: &Caller,
    state: &AppState,
    project: &str,
    plural: &'static str,
    check_op: &'static str,
    input: ManifestInput,
) -> Result<Value, OpError> {
    if let Some(d) = &input.draft {
        let draft = draft_store(state)
            .get(project, &d.kind, &d.name)
            .await
            .map_err(|e| OpError::Api(ApiError::Internal(e.to_string())))?
            .ok_or_else(|| {
                ApiError::NotFound(format!(
                    "draft '{}/{}' not found in project '{project}'",
                    d.kind, d.name
                ))
            })?;
        let warning = apply_verdict_gate(state, &draft, check_op)?;
        let mut out = mutate_manifest(
            caller,
            state,
            project,
            plural,
            draft.manifest.clone(),
            false,
            false,
        )
        .await?;
        let _ = draft_store(state).drop(project, &d.kind, &d.name).await;
        if warning {
            out["warning"] = json!("proposed without a fresh green verdict");
        }
        return Ok(out);
    }
    let manifest = input.manifest.ok_or_else(|| OpError::InvalidInput {
        path: "/manifest".into(),
        message: "either manifest or draft is required".into(),
    })?;
    propose_bare(caller, state, project, plural, manifest).await
}

/// A manifest proposed without a draft: proposed with its own check's verdict required once it has
/// passed its own checks (PF-57, T-0956), then the draft that check created is forgotten.
async fn propose_bare(
    caller: &Caller,
    state: &AppState,
    project: &str,
    plural: &'static str,
    manifest: Value,
) -> Result<Value, OpError> {
    let out = mutate_manifest(
        caller,
        state,
        project,
        plural,
        manifest.clone(),
        false,
        true,
    )
    .await?;
    forget_check(state, project, &manifest).await;
    Ok(out)
}

fn init_registry() -> Vec<Operation> {
    let mut operations = core_operations();
    operations.extend(resources::operations());
    operations.extend(runs::operations());
    operations.extend(views::operations());
    operations.extend(admin::operations());
    operations.extend(sync_sources::operations());
    operations.extend(workspaces::operations());
    operations
}

fn core_operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_catalog_search",
            title: "Search Catalog",
            description: "Find spaces, endpoints, and data models matching search keywords",
            input: catalog_search_input_schema,
            output: catalog_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                serde_json::from_value::<CatalogSearchInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |_, state, project, val| {
                Box::pin(async move {
                    let input: CatalogSearchInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    let q = input.q.or(input.query).unwrap_or_default();
                    let q = q.trim();
                    if q.is_empty() {
                        return Err(OpError::Api(ApiError::BadRequest(
                            "q must not be empty".into(),
                        )));
                    }
                    let scope = input
                        .scope
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let res = assistant::search(state, project, q, scope).await;
                    Ok(serde_json::to_value(res)?)
                })
            },
        },
        Operation {
            name: "jc_endpoint_propose",
            title: "Propose Endpoint",
            description: "Renders an Endpoint and its draft Policy manifests from a request to share data",
            input: endpoint_propose_input_schema,
            output: proposal_output_schema,
            // Parameters render a proposal and nothing is written, but a `manifest` or a
            // `draft` opens a merge request, so the annotation says what the operation can do
            // and not what its lightest path does: a client that reads `readOnlyHint` decides
            // from it whether to ask a person first (AG-07, AG-63).
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "Endpoint",
            verb: Some(Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| {
                serde_json::from_value::<EndpointProposeInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: EndpointProposeInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    if let Some(d) = &input.draft {
                        let draft = draft_store(state)
                            .get(project, &d.kind, &d.name)
                            .await
                            .map_err(|e| OpError::Api(ApiError::Internal(e.to_string())))?
                            .ok_or_else(|| {
                                ApiError::NotFound(format!(
                                    "draft '{}/{}' not found in project '{project}'",
                                    d.kind, d.name
                                ))
                            })?;
                        let warning = apply_verdict_gate(state, &draft, "jc_manifest_dry_run")?;
                        let mut out = mutate_manifest(caller, state, project, "endpoints", draft.manifest.clone(), false, false).await?;
                        let _ = draft_store(state).drop(project, &d.kind, &d.name).await;
                        if warning {
                            out["warning"] = json!("proposed without a fresh green verdict");
                        }
                        return Ok(out);
                    }
                    if let Some(manifest) = input.manifest {
                        return propose_bare(caller, state, project, "endpoints", manifest).await;
                    }
                    let params = share::ProposeEndpoint {
                        context_space: input.context_space.unwrap_or_default(),
                        name: input.name.unwrap_or_default(),
                        title: input.title,
                        audience: input.audience,
                        allowed_projects: input.allowed_projects,
                        representations: input.representations,
                        hidden_attributes: input.hidden_attributes,
                        entity_types: input.entity_types,
                        rate_limits: input.rate_limits,
                    };
                    let proposal = assistant::execute_propose_endpoint(
                        &caller.identity,
                        state,
                        project,
                        params,
                    )
                    .await?;
                    Ok(serde_json::to_value(proposal)?)
                })
            },
        },
        Operation {
            name: "jc_kpi_compute",
            title: "Compute KPI",
            description: "Folds an attribute over context entities and renders a KeyPerformanceIndicator entity",
            input: kpi_input_schema,
            output: || json!({ "type": "object" }),
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Endpoint",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                serde_json::from_value::<KpiComputeInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |_, state, project, val| {
                Box::pin(async move {
                    let input: KpiComputeInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    let params = kpi::ComputeKpi {
                        name: input.name,
                        title: input.title,
                        entity_type: input.entity_type,
                        attribute: input.attribute,
                        agg: input.agg,
                        unit: input.unit,
                        q: input.q,
                        endpoint: None,
                    };
                    let rows = input.rows.unwrap_or_default();
                    let (computed_val, _) = kpi::compute(&rows, &params.attribute, params.agg);
                    let org_domain = assistant::org_domain(state, project);
                    let (ep_slug, ep_name) = kpi::kpi_endpoint(state, project)
                        .unwrap_or_else(|| (format!("{project}-kpi"), format!("{project}-kpi")));
                    let now = chrono::Utc::now().to_rfc3339();
                    let prov = kpi::Provenance {
                        org_domain: &org_domain,
                        project,
                        endpoint_space: &ep_slug,
                        endpoint_name: &ep_name,
                        run_id: "ops",
                        now: &now,
                    };
                    let entity = kpi::entity(&params, computed_val.unwrap_or(0.0), &prov)
                        .map_err(ApiError::BadRequest)?;
                    Ok(entity.to_json())
                })
            },
        },
        Operation {
            name: "jc_manifest_dry_run",
            title: "Manifest Dry Run",
            description: "Dry-runs candidate manifest changes and returns validation result and plan diff",
            input: manifest_input_schema,
            output: dry_run_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                let input: ManifestInput = serde_json::from_value(val.clone()).map_err(|e| {
                    let (path, message) = serde_error_path_and_message(&e);
                    OpError::InvalidInput { path, message }
                })?;
                if input.manifest.is_none() && input.draft.is_none() {
                    return Err(OpError::InvalidInput {
                        path: "/manifest".into(),
                        message: "either manifest or draft is required".into(),
                    });
                }
                Ok(())
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    let (manifest, draft_ref) = resolve_manifest_input(state, project, &input).await?;
                    let res = dry_run::execute_dry_run(
                        &caller.identity,
                        state,
                        project,
                        manifest.clone(),
                    )
                    .await?;
                    let ok = res.valid;
                    let mut findings = Vec::new();
                    if !ok {
                        findings.push(Finding {
                            level: Level::Error,
                            path: "".into(),
                            message: "plan validation failed".into(),
                        });
                    }
                    let verdict = Verdict::new(
                        ok,
                        findings,
                        Some(serde_json::to_value(&res.plan).unwrap_or_default()),
                        &manifest,
                    );
                    if let Some(d) = &draft_ref {
                        record_verdict(caller, state, project, d, &manifest, &verdict).await;
                    }
                    let mut out = serde_json::to_value(&res)?;
                    out["verdict"] = serde_json::to_value(&verdict)?;
                    Ok(out)
                })
            },
        },
        Operation {
            name: "jc_pipeline_test",
            title: "Pipeline Test",
            description: "Tests candidate pipeline mapping and validation on runner without writing",
            input: pipeline_test_input_schema,
            output: pipeline_test_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: Some(Verb::Propose),
            lane: Lane::Green,
            validate: |val| {
                let input: PipelineTestInput = serde_json::from_value(val.clone()).map_err(|e| {
                    let (path, message) = serde_error_path_and_message(&e);
                    OpError::InvalidInput { path, message }
                })?;
                if input.pipeline.is_none() && input.draft.is_none() {
                    return Err(OpError::InvalidInput {
                        path: "/pipeline".into(),
                        message: "either pipeline or draft is required".into(),
                    });
                }
                Ok(())
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: PipelineTestInput =
                        serde_json::from_value(val).map_err(|e| {
                            let (path, message) = serde_error_path_and_message(&e);
                            OpError::InvalidInput { path, message }
                        })?;
                    let (pipeline, draft_ref) = if let Some(d) = &input.draft {
                        if let Some(p) = input.pipeline {
                            (p, Some(d.clone()))
                        } else {
                            let draft = draft_store(state)
                                .get(project, &d.kind, &d.name)
                                .await
                                .map_err(|e| OpError::Api(ApiError::Internal(e.to_string())))?
                                .ok_or_else(|| {
                                    ApiError::NotFound(format!(
                                        "draft '{}/{}' not found in project '{project}'",
                                        d.kind, d.name
                                    ))
                                })?;
                            (draft.manifest, Some(d.clone()))
                        }
                    } else if let Some(p) = input.pipeline {
                        let own = own_draft(&p);
                        (p, own)
                    } else {
                        return Err(OpError::InvalidInput {
                            path: "/pipeline".into(),
                            message: "either pipeline or draft is required".into(),
                        });
                    };

                    let req = pipeline_test::TestRequest {
                        pipeline: pipeline.clone(),
                        sample: input.sample,
                    };
                    let trace = pipeline_test::execute_test_pipeline(
                        &caller.identity,
                        state,
                        project,
                        req,
                    )
                    .await?;

                    let ok = trace.errors.is_empty()
                        && !trace.validation.is_empty()
                        && trace.validation.iter().all(|v| v.ok);
                    let mut findings = Vec::new();
                    for err in &trace.errors {
                        findings.push(Finding {
                            level: Level::Error,
                            path: err.stage.clone(),
                            message: err.message.clone(),
                        });
                    }
                    for val in &trace.validation {
                        if !val.ok {
                            for problem in &val.problems {
                                findings.push(Finding {
                                    level: Level::Error,
                                    path: format!("validation[{}]", val.index),
                                    message: problem.clone(),
                                });
                            }
                        }
                    }
                    let verdict = Verdict::new(
                        ok,
                        findings,
                        Some(serde_json::to_value(&trace).unwrap_or_default()),
                        &pipeline,
                    );
                    if let Some(d) = &draft_ref {
                        record_verdict(caller, state, project, d, &pipeline, &verdict).await;
                    }
                    let mut out = serde_json::to_value(&trace)?;
                    out["verdict"] = serde_json::to_value(&verdict)?;
                    Ok(out)
                })
            },
        },
        Operation {
            name: "jc_change_list",
            title: "List Changes",
            description: "Lists open change proposals and merge requests for review",
            input: empty_input_schema,
            output: change_list_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            // A Change is no manifest kind, so `access.kinds` cannot name it (MF-40) and a profile
            // could never be offered this read. Verbless, so the kind plays no part in `permitted`
            // either: the function checks that the caller may read the project (T-1475, AG-70).
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                serde_json::from_value::<EmptyInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let _: EmptyInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    let list =
                        changes::list_changes_readable(state, &caller.identity, project).await?;
                    Ok(serde_json::to_value(list)?)
                })
            },
        },
        Operation {
            name: "jc_change_approve",
            title: "Approve Change",
            description: "Approves and merges a change proposal",
            input: change_approve_input_schema,
            output: approved_change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: false,
            },
            kind: "Change",
            verb: Some(Verb::Approve),
            lane: Lane::Red,
            validate: |val| {
                serde_json::from_value::<ChangeApproveInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    resources::refuse_agent_decision(caller)?;
                    let input: ChangeApproveInput =
                        serde_json::from_value(val).map_err(|e| {
                            let (path, message) = serde_error_path_and_message(&e);
                            OpError::InvalidInput { path, message }
                        })?;
                    let change = changes::approve_change_for(
                        state,
                        &caller.identity,
                        project,
                        &input.id,
                        input.confirm.as_deref(),
                        changes::ApprovedBy::Operation,
                    )
                    .await?;
                    Ok(serde_json::to_value(change)?)
                })
            },
        },
        Operation {
            name: "jc_datasource_check",
            title: "Check DataSource",
            description: "Dry-runs a DataSource manifest and probes the external feed",
            input: manifest_input_schema,
            output: dry_run_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "DataSource",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ManifestInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = parse_input(val)?;
                    let (manifest, draft_ref) = resolve_manifest_input(state, project, &input).await?;
                    let mut out =
                        mutate_manifest(caller, state, project, "datasources", manifest.clone(), true, false).await?;
                    let verdict = datasource_verdict(&out, &manifest);
                    if let Some(d) = &draft_ref {
                        record_verdict(caller, state, project, d, &manifest, &verdict).await;
                    }
                    out["verdict"] = serde_json::to_value(&verdict)?;
                    Ok(out)
                })
            },
        },
        Operation {
            name: "jc_datasource_propose",
            title: "Propose DataSource",
            description: "Proposes creation or update of a DataSource manifest, from a draft when one is named",
            input: manifest_input_schema,
            output: change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "DataSource",
            verb: Some(Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<ManifestInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = parse_input(val)?;
                    propose_with_optional_draft(caller, state, project, "datasources", check_operation_for("jc_datasource_propose"), input).await
                })
            },
        },
        Operation {
            name: "jc_pipeline_propose",
            title: "Propose Pipeline",
            description: "Proposes creation or update of a Pipeline manifest, from a draft when one is named",
            input: manifest_input_schema,
            output: change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "Pipeline",
            verb: Some(Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<ManifestInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = parse_input(val)?;
                    propose_with_optional_draft(caller, state, project, "pipelines", check_operation_for("jc_pipeline_propose"), input).await
                })
            },
        },
        Operation {
            name: "jc_space_propose",
            title: "Propose ContextSpace",
            description: "Proposes creation or update of a ContextSpace manifest, from a draft when one is named",
            input: manifest_input_schema,
            output: change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "ContextSpace",
            verb: Some(Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<ManifestInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = parse_input(val)?;
                    propose_with_optional_draft(caller, state, project, "spaces", "jc_manifest_dry_run", input).await
                })
            },
        },
        Operation {
            name: "jc_model_propose",
            title: "Propose DataModel",
            description: "Proposes creation or update of a DataModel manifest, from a draft when one is named",
            input: manifest_input_schema,
            output: change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "DataModel",
            verb: Some(Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<ManifestInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = parse_input(val)?;
                    propose_with_optional_draft(caller, state, project, "datamodels", "jc_manifest_dry_run", input).await
                })
            },
        },
        Operation {
            name: "jc_draft_put",
            title: "Put Draft",
            description: "Writes the shared draft of a manifest every window, assistant run and MCP client sees (AG-61)",
            input: draft_put_input_schema,
            output: draft_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<DraftPutInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: DraftPutInput = parse_input(val)?;
                    crate::permissions::for_request(state, &caller.identity, project)
                        .check(&input.kind, Verb::Propose, None)?;
                    let draft = draft_store(state)
                        .put(
                            project,
                            &input.kind,
                            &input.name,
                            input.manifest,
                            input.expected_version,
                            &caller.identity.username,
                            caller.via.touched_kind(),
                        )
                        .await
                        .map_err(draft_error)?;
                    Ok(serde_json::to_value(draft)?)
                })
            },
        },
        Operation {
            name: "jc_draft_get",
            title: "Get Draft",
            description: "Reads one shared draft with its verdict (AG-61)",
            input: draft_get_input_schema,
            output: draft_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<DraftRef>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let d: DraftRef = parse_input(val)?;
                    let effective = crate::permissions::for_request(state, &caller.identity, project);
                    let draft = draft_store(state)
                        .get(project, &d.kind, &d.name)
                        .await
                        .map_err(draft_error)?
                        // Not readable is not there (PF-59, R20, T-1455).
                        .filter(|draft| effective.may_read_manifest(&draft.kind, &draft.manifest))
                        .ok_or_else(|| {
                            ApiError::NotFound(format!(
                                "draft '{}/{}' not found in project '{project}'",
                                d.kind, d.name
                            ))
                        })?;
                    Ok(serde_json::to_value(draft)?)
                })
            },
        },
        Operation {
            name: "jc_draft_list",
            title: "List Drafts",
            description: "Lists the shared drafts of a project (AG-61)",
            input: || json!({ "type": "object", "additionalProperties": false }),
            output: draft_list_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                if val.as_object().is_some_and(|o| o.is_empty()) || val.is_null() {
                    Ok(())
                } else {
                    Err(OpError::InvalidInput {
                        path: String::new(),
                        message: "no input is accepted".into(),
                    })
                }
            },
            run: |caller, state, project, _val| {
                Box::pin(async move {
                    // A draft is readable exactly where its manifest would be (PF-59, T-1455).
                    let effective = crate::permissions::for_request(state, &caller.identity, project);
                    // A line per draft, never the manifest and never the verdict's trace
                    // (T-2248): the manifest is read one at a time with jc_draft_get.
                    let items: Vec<crate::ops::drafts::DraftLine> = draft_store(state)
                        .list(project)
                        .await
                        .map_err(draft_error)?
                        .iter()
                        .filter(|draft| effective.may_read_manifest(&draft.kind, &draft.manifest))
                        .map(crate::ops::drafts::DraftLine::from)
                        .collect();
                    Ok(json!({ "items": items }))
                })
            },
        },
        Operation {
            name: "jc_draft_drop",
            title: "Drop Draft",
            description: "Discards a shared draft (AG-61)",
            input: draft_get_input_schema,
            output: draft_drop_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<DraftRef>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let d: DraftRef = parse_input(val)?;
                    crate::permissions::for_request(state, &caller.identity, project)
                        .check(&d.kind, Verb::Propose, None)?;
                    let dropped = draft_store(state)
                        .drop(project, &d.kind, &d.name)
                        .await
                        .map_err(draft_error)?;
                    Ok(json!({ "dropped": dropped }))
                })
            },
        },
        Operation {
            name: "jc_space_complete",
            title: "Complete this space",
            description: "Opens an endpoint or folder that partly defines a space and completes LinkML, data source, and pipeline drafts (MCP clients pass the files)",
            input: space_complete::input_schema,
            output: space_complete::output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "ContextSpace",
            verb: Some(Verb::Propose),
            lane: Lane::Yellow,
            validate: space_complete::validate_input,
            run: |caller, state, project, val| {
                Box::pin(async move {
                    space_complete::run(caller, state, project, val).await
                })
            },
        },
        Operation {
            name: "jc_model_infer",
            title: "Infer Schema",
            description: "Infers a draft LinkML data model from sample data bytes or text",
            input: model_infer_input_schema,
            output: || json!({ "type": "object" }),
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "DataModel",
            verb: None,
            lane: Lane::Green,
            validate: |val| {
                serde_json::from_value::<ModelInferInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |_, state, _, val| {
                Box::pin(async move {
                    let input: ModelInferInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    let bytes = if let Some(b64) = input.content {
                        base64::engine::general_purpose::STANDARD
                            .decode(b64)
                            .map_err(|e| OpError::InvalidInput {
                                path: "/content".into(),
                                message: format!("invalid base64 content: {e}"),
                            })?
                    } else if let Some(text) = input.sample {
                        text.into_bytes()
                    } else {
                        return Err(OpError::InvalidInput {
                            path: "/content".into(),
                            message: "either content (base64) or sample (text) is required".into(),
                        });
                    };
                    let res = model_tools::infer_schema_from_bytes(
                        state,
                        input.name.as_deref(),
                        &bytes,
                        input.format.as_deref(),
                    )
                    .await?;
                    Ok(res)
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_all_operations() {
        let ops = registry();
        assert_eq!(ops.len(), 63);
        for name in [
            "jc_catalog_search",
            "jc_endpoint_propose",
            "jc_kpi_compute",
            "jc_manifest_dry_run",
            "jc_pipeline_test",
            "jc_change_list",
            "jc_change_approve",
            "jc_datasource_check",
            "jc_datasource_propose",
            "jc_pipeline_propose",
            "jc_space_propose",
            "jc_model_propose",
            "jc_model_infer",
            "jc_space_complete",
            "jc_draft_put",
            "jc_draft_get",
            "jc_draft_list",
            "jc_draft_drop",
            "jc_resource_list",
            "jc_resource_get",
            "jc_resource_propose",
            "jc_resource_delete",
            "jc_change_reject",
            "jc_pipeline_metrics",
            "jc_activity_list",
            "jc_federation_graph",
            "jc_model_source_get",
            "jc_run_create",
            "jc_run_cancel",
            "jc_run_publish",
            "jc_project_create",
            "jc_project_delete",
            "jc_project_export",
            "jc_project_import",
            "jc_model_source_put",
            "jc_service_account_key_mint",
            "jc_service_account_key_rotate",
            "jc_service_account_key_revoke",
            "jc_change_get",
            "jc_run_list",
            "jc_run_get",
            "jc_service_account_key_list",
            "jc_ckan_status",
            "jc_endpoint_list_all",
            "jc_project_get",
            "jc_project_revisions",
            "jc_run_answer",
            "jc_run_message",
            "jc_flow_start",
            "jc_syncsource_status",
            "jc_syncsource_sync",
            "jc_syncsource_pause",
            "jc_syncsource_detach",
        ] {
            assert!(find(name).is_some(), "missing operation {name}");
        }
    }

    /// Every route the Portal serves, and the operation behind it (AG-59, CC-48, T-0840).
    ///
    /// The third column is the operation's name, or the sentence saying why the route has none.
    /// A route with neither fails the test below, so a new door is a decision rather than
    /// something only a browser ever knew about.
    const ROUTE_COVERAGE: &[(&str, &str, &str)] = &[
    ("GET", "/.well-known/oauth-protected-resource", "the OAuth resource metadata an MCP client reads before it authenticates"),
    ("GET", "/.well-known/oauth-protected-resource/api/v1/mcp", "the OAuth resource metadata an MCP client reads before it authenticates"),
    ("POST", "/activity", "the runner's OTLP ingest: a workload writes what happened, nobody calls it"),
    ("GET", "/apps/{name}/", "a published application, served as files"),
    ("GET", "/apps/{name}/{*path}", "a published application, served as files"),
    ("POST", "/auth/backchannel-logout", "signing out"),
    ("GET", "/auth/callback", "signing in"),
    ("GET", "/auth/login", "signing in"),
    ("POST", "/auth/logout", "signing out"),
    ("GET", "/auth/me", "who is signed in"),
    ("GET", "/blueprints", "the organisation's gallery, not a project's data; jc_flow_start runs one by name"),
    ("GET", "/branding", "the instance's look, not a project's data"),
    ("GET", "/branding/{asset}", "the instance's look, not a project's data"),
    ("GET", "/forms", "the form definitions the Portal renders, not a project's data"),
    ("GET", "/health", "liveness"),
    ("POST", "/internal/agent-runs/events", "the runner's own callback, authenticated as a workload"),
    ("GET", "/internal/agent-runs/{id}", "the runner's own callback, authenticated as a workload"),
    ("GET", "/internal/agent-runs/{id}/diagnostics/{component}/{name}", "the runner's own callback, authenticated as a workload"),
    ("GET", "/internal/agent-runs/{id}/inbox", "the runner's own callback, authenticated as a workload"),
    ("POST", "/internal/agent-runs/{id}/mcp", "the whole registry for one run, narrowed by its AgentProfile (AG-70) and refused an approval (AG-11)"),
    ("POST", "/internal/pipeline-tests/{id}", "the runner's own callback, authenticated as a workload"),
    ("GET", "/internal/previews", "the gateway's read of the running previews, admitted by NetworkPolicy alone"),
    ("GET", "/mcp", "the MCP door itself, which dispatches this registry"),
    ("POST", "/mcp", "the MCP door itself, which dispatches this registry"),
    ("GET", "/metrics", "the Prometheus scrape"),
    ("GET", "/openapi.json", "the API document"),
    ("GET", "/preferences", "this person's own Portal preferences, not a project's data"),
    ("PUT", "/preferences", "this person's own Portal preferences, not a project's data"),
    ("GET", "/endpoints", "jc_endpoint_list_all"),
    ("GET", "/projects", "the door before a project; every operation runs inside one"),
    ("POST", "/projects", "jc_project_create"),
    ("GET", "/projects/{project}", "jc_project_get"),
    ("DELETE", "/projects/{project}", "jc_project_delete"),
    ("GET", "/projects/{project}/activity", "jc_activity_list"),
    ("GET", "/projects/{project}/activity/stream", "a live stream, not a call and an answer"),
    // Drift is read and resolved on the page, not through the assistant (CC-21, UI-26): a
    // resolution is a write to the live space or to the repository, and a model proposing one
    // would be acting on a comparison it cannot see. Both are held to `propose` on `Entity`.
    ("GET", "/projects/{project}/drift", "read on the page; a scan result is not an operation"),
    ("POST", "/projects/{project}/drift/{space}/{id}/revert", "a resolution a person picks, held to propose on Entity"),
    ("POST", "/projects/{project}/drift/{space}/{id}/adopt", "a resolution a person picks, held to propose on Entity"),
    ("GET", "/projects/{project}/agent-runs", "jc_run_list"),
    ("POST", "/projects/{project}/agent-runs", "jc_run_create"),
    ("GET", "/projects/{project}/agent-runs/{id}", "jc_run_get"),
    ("POST", "/projects/{project}/agent-runs/{id}/answers", "jc_run_answer"),
    ("POST", "/projects/{project}/agent-runs/{id}/cancel", "jc_run_cancel"),
    ("GET", "/projects/{project}/agent-runs/{id}/events", "a live stream, not a call and an answer"),
    ("POST", "/projects/{project}/agent-runs/{id}/functions/{fn}", "the run calling its own tools back through the Portal"),
    ("POST", "/projects/{project}/agent-runs/{id}/messages", "jc_run_message"),
    ("GET", "/projects/{project}/agent-runs/{id}/preview", "the page that frames a run's preview, not an answer"),
    ("POST", "/projects/{project}/agent-runs/{id}/preview-errors", "the preview frame reporting its own errors"),
    ("POST", "/projects/{project}/agent-runs/{id}/preview-observations", "the preview frame reporting what it sees"),
    ("POST", "/projects/{project}/agent-runs/{id}/publish", "jc_run_publish"),
    ("GET", "/projects/{project}/assistant/access", "what the assistant may reach here; the registry's own listing answers the same question"),
    ("GET", "/projects/{project}/assistant/catalog", "jc_catalog_search"),
    ("POST", "/projects/{project}/assistant/conversations", "the assistant's own door; jc_run_create starts a conversation"),
    ("POST", "/projects/{project}/assistant/propose-endpoint", "jc_endpoint_propose"),
    ("GET", "/projects/{project}/basemap/{style}/style.json", "map tiles the browser fetches"),
    ("GET", "/projects/{project}/basemap/{style}/{z}/{x}/{tile}", "map tiles the browser fetches"),
    ("GET", "/projects/{project}/changes", "jc_change_list"),
    ("GET", "/projects/{project}/changes/{id}", "jc_change_get"),
    ("POST", "/projects/{project}/changes/{id}/approve", "jc_change_approve"),
    ("POST", "/projects/{project}/changes/{id}/reject", "jc_change_reject"),
    ("GET", "/projects/{project}/ckan/status", "jc_ckan_status"),
    ("GET", "/projects/{project}/datamodels/{name}/source", "jc_model_source_get"),
    ("PUT", "/projects/{project}/datamodels/{name}/source", "jc_model_source_put"),
    ("GET", "/projects/{project}/drafts", "jc_draft_list"),
    ("GET", "/projects/{project}/drafts/events", "a live stream, not a call and an answer"),
    ("GET", "/projects/{project}/drafts/{kind}/{name}", "jc_draft_get"),
    ("PUT", "/projects/{project}/drafts/{kind}/{name}", "jc_draft_put"),
    ("DELETE", "/projects/{project}/drafts/{kind}/{name}", "jc_draft_drop"),
    ("GET", "/projects/{project}/export", "jc_project_export"),
    ("GET", "/projects/{project}/federation-graph", "jc_federation_graph"),
    ("POST", "/projects/{project}/flows", "jc_flow_start"),
    ("POST", "/projects/{project}/import", "jc_project_import"),
    ("GET", "/projects/{project}/ops", "the registry's own listing"),
    ("POST", "/projects/{project}/ops/{name}", "the registry's own door"),
    ("GET", "/projects/{project}/permissions/me", "what this caller may do; the listing answers it per operation"),
    ("POST", "/projects/{project}/pipelines/test", "jc_pipeline_test"),
    ("GET", "/projects/{project}/pipelines/{name}/metrics", "jc_pipeline_metrics"),
    ("GET", "/projects/{project}/revisions", "jc_project_revisions"),
    ("GET", "/projects/{project}/serviceaccounts/{name}/keys", "jc_service_account_key_list"),
    ("POST", "/projects/{project}/serviceaccounts/{name}/keys", "jc_service_account_key_mint"),
    ("DELETE", "/projects/{project}/serviceaccounts/{name}/keys/{keyId}", "jc_service_account_key_revoke"),
    ("POST", "/projects/{project}/serviceaccounts/{name}/keys/{keyId}/rotate", "jc_service_account_key_rotate"),
    ("POST", "/projects/{project}/syncsources/{name}/detach", "jc_syncsource_detach"),
    ("POST", "/projects/{project}/syncsources/{name}/pause", "jc_syncsource_pause"),
    ("GET", "/projects/{project}/syncsources/{name}/status", "jc_syncsource_status"),
    ("POST", "/projects/{project}/syncsources/{name}/sync", "jc_syncsource_sync"),
    ("GET", "/projects/{project}/workspaces", "jc_workspace_list"),
    ("POST", "/projects/{project}/workspaces", "jc_workspace_open"),
    ("GET", "/projects/{project}/workspaces/{name}", "jc_workspace_get"),
    ("DELETE", "/projects/{project}/workspaces/{name}", "jc_workspace_discard"),
    ("GET", "/projects/{project}/workspaces/{name}/compare", "jc_workspace_compare"),
    ("POST", "/projects/{project}/workspaces/{name}/update", "jc_workspace_update_from_main"),
    ("POST", "/projects/{project}/workspaces/{name}/propose", "jc_workspace_propose"),
    ("POST", "/projects/{project}/workspaces/{name}/preview", "jc_workspace_preview_start"),
    ("GET", "/projects/{project}/workspaces/{name}/preview", "jc_workspace_preview_get"),
    ("DELETE", "/projects/{project}/workspaces/{name}/preview", "jc_workspace_preview_stop"),
    ("GET", "/projects/{project}/{plural}", "jc_resource_list"),
    ("POST", "/projects/{project}/{plural}", "jc_resource_propose"),
    ("GET", "/projects/{project}/{plural}/{name}", "jc_resource_get"),
    ("PUT", "/projects/{project}/{plural}/{name}", "jc_resource_propose"),
    ("PATCH", "/projects/{project}/{plural}/{name}", "jc_resource_propose"),
    ("DELETE", "/projects/{project}/{plural}/{name}", "jc_resource_delete"),
    ("GET", "/ready", "readiness"),
    ("GET", "/sync", "the mirror's own sync state: the instance, not a project"),
    ("POST", "/tools/generate", "the model tools the Portal proxies; jc_model_propose is the project's door"),
    ("POST", "/tools/import-sdm", "the model tools the Portal proxies; jc_model_propose is the project's door"),
    ("POST", "/tools/infer-schema", "the model tools the Portal proxies; jc_model_infer is the project's door"),
    ("GET", "/tools/sdm-catalog", "the model tools the Portal proxies; jc_catalog_search is the project's door"),
    ("POST", "/webhooks/gitea", "the forge calling in"),
    ("POST", "/webhooks/sync/{project}/{name}", "a foreign catalogue calling in"),
    ];

    /// Every `.route("…", …)` under `src/`, as `(METHOD, path)` with the `/api/v1` prefix off.
    ///
    /// ponytail: the source is the inventory because `axum`'s `Router` cannot be asked what it
    /// holds. A router built inside a `#[cfg(test)]` module is a fixture, so the scan stops there.
    fn routes_in_source() -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut stack = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read the crate's sources") {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read a source file");
                let text = text.split("\n#[cfg(test)]").next().unwrap_or_default();
                found.extend(routes_in(text));
            }
        }
        found.sort();
        found.dedup();
        found
    }

    fn routes_in(text: &str) -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut from = 0;
        while let Some(at) = text[from..].find(".route(") {
            let open = from + at + ".route(".len() - 1;
            let mut depth = 0usize;
            let mut end = open;
            for (offset, ch) in text[open..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let call = &text[open..=end];
            from = end + 1;
            let Some(path) = call.split('"').nth(1) else {
                continue;
            };
            let path = path.strip_prefix("/api/v1").unwrap_or(path);
            for method in ["get", "post", "put", "patch", "delete"] {
                if names_method(call, method) {
                    found.push((method.to_uppercase(), path.to_owned()));
                }
            }
        }
        found
    }

    /// `get(` as a method of this route, not the tail of a handler's name.
    fn names_method(call: &str, method: &str) -> bool {
        let needle = format!("{method}(");
        let mut from = 0;
        while let Some(at) = call[from..].find(&needle) {
            let start = from + at;
            let before = call[..start].chars().next_back().unwrap_or(' ');
            if !before.is_alphanumeric() && before != '_' {
                return true;
            }
            from = start + needle.len();
        }
        false
    }

    #[test]
    fn every_route_is_an_operation_or_says_why_it_is_not() {
        let mut expected: std::collections::HashMap<(String, String), &str> = ROUTE_COVERAGE
            .iter()
            .map(|(method, path, behind)| (((*method).to_owned(), (*path).to_owned()), *behind))
            .collect();
        assert_eq!(
            expected.len(),
            ROUTE_COVERAGE.len(),
            "ROUTE_COVERAGE names the same route twice"
        );

        for (method, path) in routes_in_source() {
            let behind = expected
                .remove(&(method.clone(), path.clone()))
                .unwrap_or_else(|| {
                    panic!(
                        "route `{method} {path}` is in no line of ROUTE_COVERAGE: give it an \
                         operation, or say there why it has none"
                    )
                });
            if behind.starts_with("jc_") {
                assert!(
                    find(behind).is_some(),
                    "route `{method} {path}` names operation `{behind}`, which the registry \
                     does not hold"
                );
            } else {
                assert!(
                    !behind.trim().is_empty(),
                    "route `{method} {path}` has no operation and no reason"
                );
            }
        }

        let mut left: Vec<&(String, String)> = expected.keys().collect();
        left.sort();
        assert!(
            left.is_empty(),
            "ROUTE_COVERAGE names routes the Portal no longer serves: {left:?}"
        );
    }

    #[test]
    fn validate_detects_unknown_fields_and_reports_path() {
        let op = find("jc_catalog_search").expect("jc_catalog_search");
        let valid = json!({ "q": "traffic" });
        assert!((op.validate)(&valid).is_ok());

        let invalid = json!({ "q": "traffic", "extraField": 123 });
        let err = (op.validate)(&invalid).expect_err("should reject unknown field");
        match err {
            OpError::InvalidInput { path, message } => {
                assert_eq!(path, "/extraField");
                assert!(message.contains("unknown field `extraField`"));
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn listing_filters_by_permission() {
        use crate::config::Config;
        use crate::permissions::ORG_NAMESPACE;
        use crate::resource::ResourceEnvelope;

        let config = Config::for_tests();
        let state = AppState::new(config, None);

        // Principal without bindings
        let viewer_id = Identity {
            subject: "f:1:viewer".into(),
            username: "viewer".into(),
            email: Some("viewer@example.sk".into()),
            name: None,
            roles: vec!["portal-viewer".into()],
            groups: vec![],
        };
        let viewer_caller = Caller::new(viewer_id, Via::Session);

        // Without a binding: the resource operations, which check the caller as their routes do.
        let list = listing(&viewer_caller, &state, "ovzdusie");
        let names: Vec<_> = list.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, resources::CHECKED_BY_THE_ROUTE);

        // Add role & binding for pipeline-developer
        let role = ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "Role".into(),
            metadata: crate::resource::ObjectMeta::new("developer", ORG_NAMESPACE),
            spec: json!({
                "rules": [
                    { "kinds": ["Pipeline", "DataSource"], "verbs": ["propose"] }
                ]
            }),
            status: None,
        };
        let binding = ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: "RoleBinding".into(),
            metadata: crate::resource::ObjectMeta::new("dev-binding", ORG_NAMESPACE),
            spec: json!({
                "role": "developer",
                "subjects": [{ "group": "devs" }],
                "scope": { "project": "ovzdusie" }
            }),
            status: None,
        };
        state.mirror.upsert(role);
        state.mirror.upsert(binding);

        let dev_id = Identity {
            subject: "f:1:dev".into(),
            username: "dev".into(),
            email: Some("dev@example.sk".into()),
            name: None,
            roles: vec![],
            groups: vec!["devs".into()],
        };
        let dev_caller = Caller::new(dev_id, Via::Session);

        let dev_list = listing(&dev_caller, &state, "ovzdusie");
        let names: Vec<_> = dev_list.iter().map(|o| o.name.as_str()).collect();

        // Should include read-only ops and propose ops for Pipeline/DataSource
        assert!(names.contains(&"jc_catalog_search"));
        assert!(names.contains(&"jc_datasource_propose"));
        assert!(names.contains(&"jc_pipeline_propose"));
        // Should NOT include propose ops for kinds not granted
        assert!(!names.contains(&"jc_space_propose"));
        assert!(!names.contains(&"jc_change_approve"));
    }
}
