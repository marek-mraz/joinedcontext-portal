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

pub mod drafts;
pub mod resources;
pub mod space_complete;
pub mod verdict;

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
use utoipa::{PartialSchema, ToSchema};

use crate::agents::kpi;
use crate::agents::share;
use crate::api::assistant;
use crate::api::changes;
use crate::api::changes::ChangeProposal;
use crate::api::dry_run;
use crate::api::mutate;
use crate::api::pipeline_test;
use crate::auth::session::Identity;
use crate::change::{Change, Lane, Operation as ChangeOp};
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
}

impl Caller {
    pub fn new(identity: Identity, via: Via) -> Self {
        Self { identity, via }
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
        (_, None) if !effective.bootstrap && effective.grants.is_empty() => {
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
fn datasource_verdict(out: &Value, manifest: &Value) -> Verdict {
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

fn draft_schema() -> Value {
    serde_json::to_value(Draft::schema()).unwrap_or_else(|_| json!({ "type": "object" }))
}

fn draft_list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": { "type": "array", "items": { "type": "object" } }
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

fn empty_input_schema() -> Value {
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

fn dry_run_output_schema() -> Value {
    let mut val = serde_json::to_value(dry_run::DryRunResult::schema())
        .unwrap_or_else(|_| json!({ "type": "object" }));
    if let Some(props) = val
        .pointer_mut("/properties")
        .and_then(Value::as_object_mut)
    {
        props.insert("verdict".into(), json!({ "type": "object" }));
    }
    val
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

fn change_proposal_schema() -> Value {
    serde_json::to_value(ChangeProposal::schema()).unwrap_or_else(|_| json!({ "type": "object" }))
}

fn change_schema() -> Value {
    serde_json::to_value(Change::schema()).unwrap_or_else(|_| json!({ "type": "object" }))
}

fn proposal_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "lane": { "type": "string" },
            "slug": { "type": "string" },
            "endpoint": { "type": "object" },
            "policies": { "type": "array" },
            "prefill": { "type": "object" }
        }
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

    let outcome = mutate::propose_with_identity(
        &caller.identity,
        state,
        project,
        plural,
        name.as_deref(),
        op,
        dry_run,
        manifest,
    )
    .await?;

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
        return Ok((m.clone(), None));
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

fn apply_verdict_gate(state: &AppState, draft: &Draft, check_op: &str) -> Result<bool, OpError> {
    let mode = verdict::get_validation_mode(state);
    let reason = match &draft.verdict {
        None => Some("verdict_absent"),
        Some(v) if !v.ok => Some("verdict_failed"),
        Some(v) if !v.is_fresh_for(&draft.manifest) => Some("stale"),
        _ => None,
    };

    if let Some(reason) = reason {
        if mode == verdict::Validation::Strict {
            let detail = match reason {
                "verdict_absent" => "The draft has not been checked; check it, then propose it.",
                "verdict_failed" => {
                    "The draft's check found problems; resolve them and check it again."
                }
                _ => "The draft changed since its check; check it again, then propose it.",
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
    mutate_manifest(caller, state, project, plural, manifest, false).await
}

fn init_registry() -> Vec<Operation> {
    let mut operations = core_operations();
    operations.extend(resources::operations());
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
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
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
                        let mut out = mutate_manifest(caller, state, project, "endpoints", draft.manifest.clone(), false).await?;
                        let _ = draft_store(state).drop(project, &d.kind, &d.name).await;
                        if warning {
                            out["warning"] = json!("proposed without a fresh green verdict");
                        }
                        return Ok(out);
                    }
                    if let Some(manifest) = input.manifest {
                        return mutate_manifest(caller, state, project, "endpoints", manifest, false).await;
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
                        (p, None)
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
            output: change_proposal_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Change",
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
            run: |_, state, project, val| {
                Box::pin(async move {
                    let _: EmptyInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    let list = changes::list_changes_for(state, project).await?;
                    Ok(serde_json::to_value(list)?)
                })
            },
        },
        Operation {
            name: "jc_change_approve",
            title: "Approve Change",
            description: "Approves and merges a change proposal",
            input: change_approve_input_schema,
            output: change_schema,
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
                    resources::refuse_agent(caller)?;
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
                        mutate_manifest(caller, state, project, "datasources", manifest.clone(), true).await?;
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
                    propose_with_optional_draft(caller, state, project, "datasources", "jc_datasource_check", input).await
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
                    propose_with_optional_draft(caller, state, project, "pipelines", "jc_pipeline_test", input).await
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
            run: |_caller, state, project, val| {
                Box::pin(async move {
                    let d: DraftRef = parse_input(val)?;
                    let draft = draft_store(state)
                        .get(project, &d.kind, &d.name)
                        .await
                        .map_err(draft_error)?
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
            run: |_caller, state, project, _val| {
                Box::pin(async move {
                    let items = draft_store(state).list(project).await.map_err(draft_error)?;
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
        assert_eq!(ops.len(), 23);
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
        ] {
            assert!(find(name).is_some(), "missing operation {name}");
        }
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
