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

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use jc_core::kinds::Verb;
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
    let effective = crate::permissions::for_request(state, &caller.identity, project);
    if let Some(verb) = op.verb {
        effective.check(op.kind, verb, None)?;
    } else if !effective.bootstrap && effective.grants.is_empty() {
        return Err(OpError::Api(ApiError::Denied(format!(
            "no role grants access in project {project} (PF-50)"
        ))));
    }

    (op.validate)(&input)?;
    (op.run)(caller, state, project, input).await
}

pub fn listing(caller: &Caller, state: &AppState, project: &str) -> Vec<OperationSummary> {
    let effective = crate::permissions::for_request(state, &caller.identity, project);
    if !effective.bootstrap && effective.grants.is_empty() {
        return Vec::new();
    }
    registry()
        .iter()
        .filter(|op| {
            if effective.bootstrap {
                return true;
            }
            match op.verb {
                Some(verb) => effective.check(op.kind, verb, None).is_ok(),
                None => true,
            }
        })
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
    pub manifest: Value,
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
            "manifest": { "type": "object" }
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
            "manifest": { "type": "object" }
        },
        "required": ["manifest"],
        "additionalProperties": false
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
            }
        },
        "required": ["pipeline", "sample"],
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
    serde_json::to_value(dry_run::DryRunResult::schema())
        .unwrap_or_else(|_| json!({ "type": "object" }))
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

fn init_registry() -> Vec<Operation> {
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
                serde_json::from_value::<ManifestInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    let res = dry_run::execute_dry_run(
                        &caller.identity,
                        state,
                        project,
                        input.manifest,
                    )
                    .await?;
                    Ok(serde_json::to_value(res)?)
                })
            },
        },
        Operation {
            name: "jc_pipeline_test",
            title: "Pipeline Test",
            description: "Tests candidate pipeline mapping and validation on runner without writing",
            input: pipeline_test_input_schema,
            output: || json!({ "type": "object" }),
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: Some(Verb::Propose),
            lane: Lane::Green,
            validate: |val| {
                serde_json::from_value::<pipeline_test::TestRequest>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: pipeline_test::TestRequest =
                        serde_json::from_value(val).map_err(|e| {
                            let (path, message) = serde_error_path_and_message(&e);
                            OpError::InvalidInput { path, message }
                        })?;
                    let trace = pipeline_test::execute_test_pipeline(
                        &caller.identity,
                        state,
                        project,
                        input,
                    )
                    .await?;
                    Ok(serde_json::to_value(trace)?)
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
            validate: |val| {
                serde_json::from_value::<ManifestInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    mutate_manifest(caller, state, project, "datasources", input.manifest, true).await
                })
            },
        },
        Operation {
            name: "jc_datasource_propose",
            title: "Propose DataSource",
            description: "Proposes creation or update of a DataSource manifest",
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
            validate: |val| {
                serde_json::from_value::<ManifestInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    mutate_manifest(caller, state, project, "datasources", input.manifest, false).await
                })
            },
        },
        Operation {
            name: "jc_pipeline_propose",
            title: "Propose Pipeline",
            description: "Proposes creation or update of a Pipeline manifest",
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
            validate: |val| {
                serde_json::from_value::<ManifestInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    mutate_manifest(caller, state, project, "pipelines", input.manifest, false).await
                })
            },
        },
        Operation {
            name: "jc_space_propose",
            title: "Propose ContextSpace",
            description: "Proposes creation or update of a ContextSpace manifest",
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
            validate: |val| {
                serde_json::from_value::<ManifestInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    mutate_manifest(caller, state, project, "spaces", input.manifest, false).await
                })
            },
        },
        Operation {
            name: "jc_model_propose",
            title: "Propose DataModel",
            description: "Proposes creation or update of a DataModel manifest",
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
            validate: |val| {
                serde_json::from_value::<ManifestInput>(val.clone())
                    .map(|_| ())
                    .map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ManifestInput = serde_json::from_value(val).map_err(|e| {
                        let (path, message) = serde_error_path_and_message(&e);
                        OpError::InvalidInput { path, message }
                    })?;
                    mutate_manifest(caller, state, project, "datamodels", input.manifest, false).await
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
        assert_eq!(ops.len(), 13);
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

        let list = listing(&viewer_caller, &state, "ovzdusie");
        assert!(list.is_empty(), "caller with no binding gets empty list");

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
