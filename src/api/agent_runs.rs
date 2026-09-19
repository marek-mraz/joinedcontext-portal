//! Builder runs: the routes a person drives an agent through, and the two the credential proxy
//! calls back on (AG-43…AG-46, AG-52, AP-44, AP-46, AP-51, AP-55).
//!
//! The Portal holds everything a run is: what was asked for, what the agent may reach, how much
//! it may spend, and every line of the conversation. The workspace holds a ticket and nothing
//! else (ADR-N-020), so these routes are the whole of what a run can do to the platform.
//!
//! Two surfaces, one module. The project routes are the Portal's public API and carry a session
//! or a bearer. The `internal/` pair is served on a separate listener the edge does not route
//! (`JC_INTERNAL_BIND`) and is authenticated by the proxy's own token: `runId` on an event comes
//! from the run whose ticket the proxy verified, which is what makes AG-46 hold.

use std::collections::BTreeMap;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use utoipa::{IntoParams, ToSchema};

use crate::agents::kube;
use crate::agents::needs::validate_data_needs;
use crate::agents::profile::Profile;
use crate::agents::run::{
    digest_prompt, mint_run_id, mint_ticket, AgentRun, AgentRunEvent, AgentRunStatus,
};
use crate::agents::store::{now_rfc3339, StatusChangeError, StoreError};
use crate::agents::{kit, oneshot, preview, transpile};
use crate::auth::session::{Front, EDGE_TOKEN_HEADER};
use crate::auth::CurrentUser;
use crate::config::AgentSettings;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// How many runs one list answer carries at most.
const MAX_LIST_LIMIT: i64 = 100;
const DEFAULT_LIST_LIMIT: i64 = 20;
/// Longest prompt a run is started from. A prompt is a paragraph, not a corpus; the brief the
/// agent works from is the manifests and the model, not this field.
pub(crate) const MAX_PROMPT_CHARS: usize = 4_000;
/// Longest instruction a person may send into a live run. Same ceiling as the prompt: it is a
/// sentence of steering, and a workspace reads it with the same budget as everything else.
const MAX_MESSAGE_CHARS: usize = 4_000;
/// Longest runtime error a preview frame may report, and the longest file name it may name.
const MAX_PREVIEW_ERROR_CHARS: usize = 2_000;
const MAX_PREVIEW_FILE_CHARS: usize = 256;
/// The bounds of a preview observation (API/04 §5, SDK-27).
const MAX_OBSERVED_PAGES: usize = 20;
const MAX_PAGE_LABEL_CHARS: usize = 120;
const MAX_PAGE_TEXT_CHARS: usize = 20_000;
const MAX_OBSERVED_ENTRIES: usize = 50;
/// How long the workspace's inbox call waits for something new before answering empty. Short
/// enough to sit inside every proxy's read timeout, long enough that an idle agent is not a
/// request per second.
const INBOX_WAIT_SECS: u64 = 25;
/// The kinds a workspace reads from its inbox: what the person answered, and what they said.
const INBOX_KINDS: [&str; 2] = ["answer", "message"];
/// The run one application came out of, on the `App` manifest a publish opens (AP-51).
const AGENT_RUN_ANNOTATION: &str = "joinedcontext.com/agent-run";
/// The digest of the prompt that run was started from.
const PROMPT_DIGEST_ANNOTATION: &str = "joinedcontext.com/prompt-digest";
/// The toolchains a generated application is built with (AP-11), the same pins
/// `Architecture/16` writes into its worked examples.
const RUST_TOOLCHAIN: &str = "1.90";
const NODE_TOOLCHAIN: &str = "22";

/// The `: keep-alive` comment interval of the event stream, short enough for an edge's read
/// timeout to never see an idle stream.
const KEEP_ALIVE_SECS: u64 = 15;

/// What a person asks for when they start a run (AP-51).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateRunRequest {
    /// Name of the application to build; becomes the `App` manifest's name.
    pub app_name: String,
    /// The one `Endpoint` the application reads through; `endpointNames` for several. Nothing
    /// else is reachable.
    #[serde(default)]
    pub endpoint_name: Option<String>,
    /// The `Endpoint`s the application reads, one to five, the first the primary (AP-44).
    #[serde(default)]
    pub endpoint_names: Vec<String>,
    /// Which `AgentProfile` runs. Defaults to the builder profile the platform ships.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// `static`, `service` or `fullstack`, as the `App` kind spells them.
    pub app_class: String,
    /// Who may reach the published application. `public` is refused (AP-42).
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// What kind of run to execute: application, dashboard, analysis.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Whether the run executes unattended without interactive questions.
    #[serde(default)]
    pub unattended: bool,
    /// What the application should do, in the person's own words.
    pub prompt: String,
    /// The types, attributes and operations the application needs. Checked against what the
    /// endpoint publishes before anything is scheduled (AP-44).
    pub data_needs: Vec<serde_json::Value>,
}

pub(crate) fn default_profile() -> String {
    "app-builder".to_owned()
}

fn default_visibility() -> String {
    "project".to_owned()
}

fn default_kind() -> String {
    "application".to_owned()
}

/// Query parameters for listing runs.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListRunsQuery {
    pub limit: Option<i64>,
    pub app: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub mine: Option<bool>,
}

/// One page of runs, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct RunList {
    pub items: Vec<AgentRun>,
}

/// An answer to one question the agent asked (AG-45).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AnswerRequest {
    pub question_id: String,
    pub answers: serde_json::Value,
}

/// What the proxy relays on behalf of a workspace it has already authenticated.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RelayedEvent {
    pub run_id: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// What a person says to a run that is already going (AG-45).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageRequest {
    pub text: String,
    /// On a conversation: the endpoints the assistant may query from this message on (AG-75).
    #[serde(default, rename = "endpointNames")]
    pub endpoint_names: Option<Vec<String>>,
}

/// A runtime error the preview frame posted as `jc-error`, relayed by the page that frames it.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewErrorRequest {
    pub message: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
}

/// What the preview frame saw of one version, page by page, relayed by the page that frames it
/// (SDK-27). Every field is text the frame wrote.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewObservationRequest {
    /// The `v` of the preview URL the frame loaded.
    pub version: u32,
    pub pages: Vec<ObservedPage>,
    #[serde(default)]
    pub failed_requests: Vec<FailedRequest>,
}

/// One page of the application as the frame rendered it.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservedPage {
    pub label: String,
    /// The page's visible text.
    pub text: String,
    /// The row counts of its tables.
    #[serde(default)]
    pub rows: Vec<u32>,
}

/// A request of the frame the bridge answered with an error status.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FailedRequest {
    pub path: String,
    pub status: u16,
}

/// Where the workspace reads from, how far it has read, and how long it will wait.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxQuery {
    #[serde(default)]
    pub after: i64,
    /// Seconds to hold the call open when there is nothing new, clamped to `INBOX_WAIT_SECS`.
    /// `0` answers at once, which is what an agent between two steps of its own work wants.
    pub wait: Option<u64>,
}

/// The answers and instructions after `after`, oldest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct Inbox {
    pub items: Vec<RelayedInbound>,
}

/// One line of the inbox. The payload is the person's own words, so a workspace treats it as
/// data: it is an instruction to the agent, never a grant, and nothing here widens `dataNeeds`.
#[derive(Debug, Serialize, ToSchema)]
pub struct RelayedInbound {
    pub seq: i64,
    pub kind: String,
    pub payload: serde_json::Value,
}

/// The sequence number a relayed event was given.
#[derive(Debug, Serialize, ToSchema)]
pub struct EventReceipt {
    pub seq: i64,
}

/// Everything the proxy needs to decide one request, and nothing a workspace may see.
///
/// This is the one place the ticket hash leaves the Portal, and it leaves on the internal
/// listener alone. The budgets are read from the profile at answer time rather than copied into
/// the run row, so lowering a profile's ceiling takes effect on the runs already in flight.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunContext {
    pub id: String,
    pub project: String,
    pub app_name: String,
    pub endpoint_slug: String,
    /// Every endpoint slug of the run, the primary first: what `/v1/data/endpoints/{slug}/…`
    /// may address (Architecture/19 §4).
    pub endpoint_slugs: Vec<String>,
    pub allows_write: bool,
    pub branch: String,
    pub path_prefix: String,
    pub status: String,
    pub ticket_hash: String,
    pub max_tokens: u64,
    pub allowed_hosts: Vec<String>,
    pub requests_per_minute: u32,
    /// The profile's `limits.stepsPerRun`: the proxy counts one model call per step and refuses
    /// the call past it, so a run stops at the limit however its driver loops (AG-25, AG-51).
    pub steps_per_run: u32,
    pub max_response_bytes: u64,
    /// The profile's `egress.maxBytesPerRun`: what the run may read from the allow-listed hosts
    /// in total, counted by the proxy's fetch route. Zero for a profile that names no host, and
    /// a run with zero reaches nothing (AG-50, AG-65).
    pub max_egress_bytes_per_run: u64,
    pub created_by: String,
    pub model_name: String,
    /// The profile's `model.reasoningEffort`: the proxy adds it to every model call (AG-72).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// The ticket, handed to the workspace and to nobody else. It is in the create answer because
/// the caller is the Portal's own UI in the one deployment that has no cluster to schedule in;
/// in a cluster the Job env is the only carrier and this field is absent.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedRun {
    #[serde(flatten)]
    pub run: AgentRun,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs",
    tag = "agents",
    params(("project" = String, Path, description = "Project name")),
    request_body = CreateRunRequest,
    responses(
        (status = 202, description = "The run, queued", body = CreatedRun),
        (status = 400, description = "A request the endpoint or the policy does not allow", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "No role grants proposing an App here", body = ProblemDetails),
        (status = 404, description = "No such endpoint in this project", body = ProblemDetails),
        (status = 503, description = "No agent runner, or no such builder profile", body = ProblemDetails)
    )
)]
pub async fn create_run(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<CreatedRun>), ApiError> {
    let settings = agent_settings(&state)?;

    // Proposing the App this run will write is the permission a run needs; the merge request at
    // the end is reviewed like any other (AP-10, PF-50).
    crate::permissions::for_request(&state, &user.0.identity, &project).check(
        "App",
        jc_core::kinds::Verb::Propose,
        None,
    )?;

    within_runs_per_day(&state, &project).await?;

    if !is_dns1123(&request.app_name) {
        return Err(ApiError::BadRequest(format!(
            "appName '{}' is not a DNS-1123 label",
            request.app_name
        )));
    }
    if request.prompt.trim().is_empty() {
        return Err(ApiError::BadRequest("prompt must not be empty".into()));
    }
    if request.prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(ApiError::BadRequest(format!(
            "prompt is longer than {MAX_PROMPT_CHARS} characters"
        )));
    }
    // The two enumerations are jc-core's, not a second copy of them: what the `App` kind
    // accepts is what a run may be started for.
    serde_json::from_value::<jc_core::kinds::AppClass>(serde_json::json!(request.app_class))
        .map_err(|_| {
            ApiError::BadRequest(format!(
                "appClass '{}' is not one of static, service, fullstack",
                request.app_class
            ))
        })?;
    serde_json::from_value::<jc_core::kinds::AppVisibility>(serde_json::json!(request.visibility))
        .map_err(|_| {
            ApiError::BadRequest(format!(
                "visibility '{}' is not one of private, project, organization, public",
                request.visibility
            ))
        })?;

    let profile = Profile::load(&state.mirror, &request.profile)?;
    if !profile.is_builder() {
        return Err(ApiError::BadRequest(format!(
            "agent profile '{}' has role '{}' and builds no application (AG-26)",
            profile.name, profile.role
        )));
    }

    if request.kind == "conversation" {
        return Err(ApiError::BadRequest(
            "a conversation starts at /assistant/conversations".into(),
        ));
    }
    if !crate::agents::run::RUN_KINDS.contains(&request.kind.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "kind '{}' is not one of {}",
            request.kind,
            crate::agents::run::RUN_KINDS.join(", ")
        )));
    }
    let unattended = if request.kind == "dashboard" || request.kind == "analysis" {
        true
    } else {
        request.unattended
    };

    // The endpoints first: every name the project has, with a slug, at most five (AP-44).
    let names = crate::agents::endpoints::requested(
        request.endpoint_name.as_deref(),
        &request.endpoint_names,
    )
    .map_err(ApiError::BadRequest)?;
    let run_endpoints = crate::agents::endpoints::resolve(&state.mirror, &project, &names)?;

    // AP-42 and AP-44 in one place: public is refused, and every violation of what the
    // endpoints publish is named at once.
    let allows_write = validate_data_needs(
        &state.mirror,
        &project,
        &run_endpoints,
        &request.visibility,
        &request.data_needs,
        &user,
    )?;

    // AG-70: a profile that lists endpoints builds only on the ones it grants, with write for a
    // run that writes.
    if let Some(refused) = run_endpoints
        .iter()
        .find(|endpoint| !profile.access.grants_endpoint(&endpoint.name, allows_write))
    {
        return Err(ApiError::Denied(format!(
            "agent profile '{}' does not grant {} on endpoint '{}' (AG-70)",
            profile.name,
            if allows_write {
                "read and write"
            } else {
                "read"
            },
            refused.name
        )));
    }

    let endpoint_slug = run_endpoints[0].slug.clone();

    if request.kind != "conversation" {
        if let Some(live) = state
            .agents
            .live_run_for_app(&project, &request.app_name)
            .await
            .map_err(unavailable)?
        {
            return Err(ApiError::Conflict(format!(
                "application '{}' already has a live run: {}",
                request.app_name, live.id
            )));
        }
    }

    let id = mint_run_id();
    let (ticket, ticket_hash) = mint_ticket();
    let created_at = now_rfc3339();
    let expires_at = expiry(settings.run_ttl_secs);
    let run = AgentRun {
        id: id.clone(),
        project: project.clone(),
        app_name: request.app_name.clone(),
        title: None,
        endpoint_name: run_endpoints[0].name.clone(),
        endpoint_slug,
        endpoints: serde_json::to_value(&run_endpoints).unwrap_or_else(|_| serde_json::json!([])),
        profile: profile.name.clone(),
        kind: request.kind.clone(),
        unattended,
        continues: None,
        app_class: request.app_class.clone(),
        visibility: request.visibility.clone(),
        prompt: request.prompt.clone(),
        prompt_digest: digest_prompt(&request.prompt),
        data_needs: serde_json::Value::Array(request.data_needs.clone()),
        allows_write,
        branch: format!("agent/app-{}/{}", request.app_name, id),
        path_prefix: format!("projects/{project}/apps/{}/", request.app_name),
        status: AgentRunStatus::Queued.as_str().to_owned(),
        ticket_hash,
        workspace: None,
        merge_request: None,
        change_id: None,
        source_url: None,
        preview_url: None,
        first_frame_ms: None,
        first_version_ms: None,
        files: serde_json::Value::Object(serde_json::Map::new()),
        steps: 0,
        tokens_used: 0,
        created_by: user.0.identity.username.clone(),
        starter: serde_json::to_value(&user.0.identity).unwrap_or(serde_json::Value::Null),
        created_at,
        started_at: None,
        finished_at: None,
        expires_at,
        error: None,
    };

    state.agents.create_run(&run).await.map_err(unavailable)?;
    publish_event(
        &state,
        &id,
        "status",
        status_payload(AgentRunStatus::Queued),
    )
    .await?;

    // A static application is the kit pass: the Portal drives it itself, in this process, and
    // the ticket stays with the driver (AP-56, AG-54). The workspace Job is what the other two
    // classes get.
    if request.app_class == "static" {
        oneshot::spawn(
            state.clone(),
            &run,
            &user.0.identity,
            &ticket,
            &profile,
            settings,
            crate::agents::oneshot::FormContext::default(),
        );
        return Ok((StatusCode::ACCEPTED, Json(CreatedRun { run, ticket: None })));
    }

    // The workspace is where the ticket goes. Scheduling it is the last step, so a run that
    // could not be recorded never has a pod: the pod is what spends money.
    match kube::schedule_workspace_job(
        state.kube.as_deref(),
        &settings.namespace,
        &settings.portal_namespace,
        &run,
        &ticket,
        &settings.proxy_base,
        &profile,
        settings.run_ttl_secs,
    )
    .await
    {
        Ok(scheduled) => {
            if scheduled {
                let _ = state
                    .agents
                    .set_status(&id, AgentRunStatus::Starting, None)
                    .await;
                publish_event(
                    &state,
                    &id,
                    "status",
                    status_payload(AgentRunStatus::Starting),
                )
                .await?;
            }
            let run = state
                .agents
                .get_run(&id)
                .await
                .map_err(unavailable)?
                .unwrap_or(run);
            Ok((
                StatusCode::ACCEPTED,
                Json(CreatedRun {
                    run,
                    // Nowhere to schedule means the caller drives the workspace itself, and
                    // then it needs the ticket; a scheduled run's ticket stays in the Job.
                    ticket: (!scheduled).then_some(ticket),
                }),
            ))
        }
        Err(message) => {
            let _ = state
                .agents
                .set_status(&id, AgentRunStatus::Failed, Some(&message))
                .await;
            publish_event(
                &state,
                &id,
                "status",
                serde_json::json!({
                    "status": AgentRunStatus::Failed.as_str(),
                    "error": message,
                }),
            )
            .await?;
            Err(ApiError::Unavailable(format!(
                "the workspace could not be scheduled: {message}"
            )))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/agent-runs",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ListRunsQuery,
    ),
    responses(
        (status = 200, description = "The project's runs, newest first", body = RunList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "The run store did not answer", body = ProblemDetails)
    )
)]
pub async fn list_runs(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ListRunsQuery>,
) -> Result<Json<RunList>, ApiError> {
    // The runs of a project the caller may not read are the runs of no project (PF-59, R20,
    // T-1368), not an empty list that says the project is there.
    if !crate::permissions::for_request(&state, &user.0.identity, &project).may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let limit = match query.limit {
        Some(n) if n <= 0 => return Err(ApiError::BadRequest("limit must be positive".into())),
        Some(n) => n.min(MAX_LIST_LIMIT),
        None => DEFAULT_LIST_LIMIT,
    };
    let is_approver =
        crate::api::changes::may_approve_anything(&state, &user.0.identity, &project).is_ok();
    let created_by = if !is_approver || query.mine == Some(true) {
        Some(user.0.identity.username.clone())
    } else {
        None
    };
    let filter = crate::agents::store::RunFilter {
        app: query.app,
        kind: query.kind,
        status: query.status,
        created_by,
    };
    let items = state
        .agents
        .list_runs_filtered(&project, &filter, limit)
        .await
        .map_err(unavailable)?;
    let items = items
        .into_iter()
        .map(|run| with_links(&state, run))
        .collect();
    Ok(Json(RunList { items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/agent-runs/{id}",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    responses(
        (status = 200, description = "The run", body = AgentRun),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails)
    )
)]
pub async fn get_run(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<AgentRun>, ApiError> {
    Ok(Json(with_links(
        &state,
        own_run_of(&state, &user, &project, &id).await?,
    )))
}

/// The run with the links a person follows from it: its source in Git and the Change that
/// publishes it (AP-71). The forge's public address, never a cluster-internal name.
pub(crate) fn with_links(state: &AppState, mut run: AgentRun) -> AgentRun {
    run.change_id = run
        .merge_request
        .and_then(|number| u64::try_from(number).ok())
        .map(|number| crate::change::ChangeMeta::from_merge_request(number, "").name);
    if let Some(gitea) = state.gitea.as_deref() {
        if !run.branch.is_empty() {
            run.source_url = Some(gitea.browse_url(&run.path_prefix, &run.branch));
        }
    }
    run
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/agent-runs/{id}/events",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
        ("Last-Event-ID" = Option<i64>, Header, description = "Resume after this sequence number"),
    ),
    responses(
        (status = 200, description = "The run's event stream, text/event-stream"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails)
    )
)]
pub async fn stream_events(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    own_run_of(&state, &user, &project, &id).await?;
    let after = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);

    // Subscribe before reading the backlog: an event published between the two is then in the
    // live half rather than in neither, and the sequence number filter drops the overlap.
    let live = state.agent_events.subscribe(&id).await;
    let replay = state
        .agents
        .events_since(&id, after)
        .await
        .map_err(unavailable)?;
    let replayed_through = replay.last().map(|event| event.seq).unwrap_or(after);

    let backlog = tokio_stream::iter(
        replay
            .into_iter()
            .map(|event| Ok::<Event, std::convert::Infallible>(sse(&event))),
    );
    let tail = BroadcastStream::new(live).filter_map(
        move |item| -> Option<Result<Event, std::convert::Infallible>> {
            match item {
                Ok(event) if event.seq > replayed_through => Some(Ok(sse(&event))),
                // Already in the backlog this stream just sent.
                Ok(_) => None,
                // A browser that fell behind is told so rather than handed a stream with a hole in it;
                // its reconnect carries `Last-Event-ID` and the backlog fills the gap.
                Err(BroadcastStreamRecvError::Lagged(missed)) => Some(Ok(Event::default()
                    .event("lag")
                    .data(serde_json::json!({ "missed": missed }).to_string()))),
            }
        },
    );

    let stream = Sse::new(backlog.chain(tail)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(KEEP_ALIVE_SECS))
            .text("keep-alive"),
    );

    let mut response = stream.into_response();
    // The edge buffers a proxied response by default, which would hold every event until the
    // run ended (AG-45).
    response.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        header::HeaderValue::from_static("no"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/answers",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    request_body = AnswerRequest,
    responses(
        (status = 204, description = "The answer is on the run's log"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run is over", body = ProblemDetails),
        (status = 400, description = "The answer is not one of what the question offered", body = ProblemDetails)
    )
)]
pub async fn answer_question(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    Json(request): Json<AnswerRequest>,
) -> Result<StatusCode, ApiError> {
    let run = own_run_of(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and asks nothing",
            run.status
        )));
    }
    let asked = state
        .agents
        .events_since(&id, 0)
        .await
        .map_err(unavailable)?
        .into_iter()
        .rev()
        .find(|event| {
            event.kind == "question"
                && event.payload.get("questionId").and_then(|q| q.as_str())
                    == Some(request.question_id.as_str())
        });
    if let Some(asked) = &asked {
        offered_answer(&asked.payload, &request.answers).map_err(ApiError::BadRequest)?;
    }
    publish_event(
        &state,
        &id,
        "answer",
        serde_json::json!({
            "questionId": request.question_id,
            "answers": request.answers,
            "answeredBy": user.0.identity.username,
        }),
    )
    .await?;
    record_answer(&state, &project, &id, &user, &request.question_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// An answer to a question the Portal filled or that takes several answers is one of what it
/// offered, never what a browser made up (AG-83, UI-73): a value that was not offered, a repeat
/// or a count outside `min`/`max` is refused and the question stays open. A question of the
/// model's own options with one answer keeps "Something else…" (UI-57).
fn offered_answer(question: &serde_json::Value, answers: &serde_json::Value) -> Result<(), String> {
    use serde_json::Value;
    let picked = question.get("pick").is_some_and(|pick| !pick.is_null());
    let multiple = question.get("multiple").and_then(Value::as_bool) == Some(true);
    if !picked && !multiple {
        return Ok(());
    }
    let offered: Vec<&str> = question
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| option.get("value").and_then(Value::as_str))
        .collect();
    let answer = answers.get("answer");
    let chosen: Vec<&str> = if multiple {
        let many = answer
            .and_then(Value::as_array)
            .ok_or("the question takes a list of answers")?;
        many.iter()
            .map(|one| one.as_str().ok_or("every answer is one of the options"))
            .collect::<Result<_, _>>()?
    } else {
        vec![answer
            .and_then(Value::as_str)
            .ok_or("the question takes one of its options")?]
    };
    if let Some(stranger) = chosen.iter().find(|one| !offered.contains(one)) {
        return Err(format!("'{stranger}' is not one of the options offered"));
    }
    let mut unique = chosen.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != chosen.len() {
        return Err("an option is chosen twice".to_owned());
    }
    let count = |key: &str| question.get(key).and_then(Value::as_u64);
    if let Some(min) = count("min") {
        if (chosen.len() as u64) < min {
            return Err(format!("choose at least {min}"));
        }
    }
    if let Some(max) = count("max") {
        if (chosen.len() as u64) > max {
            return Err(format!("choose at most {max}"));
        }
    }
    Ok(())
}

/// A person answered a run's question, on the project's activity (AG-80, OPS-48).
///
/// What they typed stays on the run's timeline, where the driver redacts it; the activity feed
/// says only that the question was answered, by whom, so a typed password never reaches a
/// projection built for reading. Failing to record it does not fail the answer: the timeline and
/// the audit log already hold it, and the activity store is derived from them (OPS-49).
async fn record_answer(
    state: &AppState,
    project: &str,
    run_id: &str,
    user: &CurrentUser,
    question_id: &str,
) {
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "agent.answer".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} answered a question of run {run_id}",
            user.0.identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: serde_json::json!({
            "object": format!("agent-runs/{run_id}"),
            "runId": run_id,
            "questionId": question_id,
            "subject": user.0.identity.subject,
        }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(run = run_id, error = %err, "the answer is not on the activity feed");
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/messages",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    request_body = MessageRequest,
    responses(
        (status = 204, description = "The instruction is on the run's log and in its inbox"),
        (status = 400, description = "An empty or over-long instruction", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run is over and reads nothing", body = ProblemDetails)
    )
)]
pub async fn post_message(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    Json(request): Json<MessageRequest>,
) -> Result<StatusCode, ApiError> {
    let run = own_run_of(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and reads nothing",
            run.status
        )));
    }
    let text = request.text.trim();
    if text.is_empty() {
        return Err(ApiError::BadRequest("text must not be empty".into()));
    }
    if text.chars().count() > MAX_MESSAGE_CHARS {
        return Err(ApiError::BadRequest(format!(
            "text is longer than {MAX_MESSAGE_CHARS} characters"
        )));
    }
    if let Some(names) = &request.endpoint_names {
        if run.kind != "conversation" {
            return Err(ApiError::BadRequest(
                "endpointNames changes the endpoints of a conversation only".into(),
            ));
        }
        let endpoints = if names.is_empty() {
            Vec::new()
        } else {
            let names =
                crate::agents::endpoints::requested(None, names).map_err(ApiError::BadRequest)?;
            crate::agents::endpoints::resolve(&state.mirror, &project, &names)?
        };
        let profile = crate::agents::profile::Profile::load(&state.mirror, &run.profile)?;
        if let Some(refused) = endpoints
            .iter()
            .find(|endpoint| !profile.access.grants_endpoint(&endpoint.name, false))
        {
            return Err(ApiError::Denied(format!(
                "agent profile '{}' does not grant reading endpoint '{}' (AG-70)",
                profile.name, refused.name
            )));
        }
        let changed = crate::agents::endpoints::of_run(&run) != endpoints;
        if changed {
            state
                .agents
                .set_endpoints(&id, &endpoints)
                .await
                .map_err(unavailable)?;
            publish_event(
                &state,
                &id,
                "endpoints",
                serde_json::json!({
                    "names": endpoints.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
                }),
            )
            .await?;
        }
    }
    publish_event(
        &state,
        &id,
        "message",
        serde_json::json!({
            "text": text,
            "sentBy": user.0.identity.username,
        }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/preview-errors",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    request_body = PreviewErrorRequest,
    responses(
        (status = 204, description = "The error is on the run's log as a preview_error event"),
        (status = 400, description = "A blank or over-long message, an over-long file, or line 0", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run is over", body = ProblemDetails)
    )
)]
/// A runtime error of the preview, for the first run's repair and the editing agent's
/// `preview_errors` tool (SDK-14, SDK-20). The frame wrote every field: it is stored as text.
pub async fn post_preview_error(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    Json(request): Json<PreviewErrorRequest>,
) -> Result<StatusCode, ApiError> {
    let run = own_run_of(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and repairs nothing",
            run.status
        )));
    }
    let message = request.message.trim();
    if message.is_empty() || message.chars().count() > MAX_PREVIEW_ERROR_CHARS {
        return Err(ApiError::BadRequest(format!(
            "message must be 1 to {MAX_PREVIEW_ERROR_CHARS} characters"
        )));
    }
    if request
        .file
        .as_ref()
        .is_some_and(|file| file.chars().count() > MAX_PREVIEW_FILE_CHARS)
    {
        return Err(ApiError::BadRequest(format!(
            "file is longer than {MAX_PREVIEW_FILE_CHARS} characters"
        )));
    }
    if request.line == Some(0) {
        return Err(ApiError::BadRequest("line starts at 1".into()));
    }
    publish_event(
        &state,
        &id,
        "preview_error",
        serde_json::json!({
            "message": message,
            "file": request.file,
            "line": request.line,
            "reportedBy": user.0.identity.username,
        }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/preview-observations",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    request_body = PreviewObservationRequest,
    responses(
        (status = 204, description = "The observation is on the run's log as a preview_observation event"),
        (status = 400, description = "A value outside the bounds of API/04 §5", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run is over, or this version was observed already", body = ProblemDetails)
    )
)]
/// What the preview frame saw of one version, for the run's verification (SDK-27, SDK-28). The
/// first observation of a version counts; a second one is refused, so a reload does not start a
/// second check of the same files.
pub async fn post_preview_observation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    Json(request): Json<PreviewObservationRequest>,
) -> Result<StatusCode, ApiError> {
    let run = own_run_of(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and verifies nothing",
            run.status
        )));
    }
    if let Some(problem) = observation_out_of_bounds(&request) {
        return Err(ApiError::BadRequest(problem));
    }
    let observed = state
        .agents
        .events_since(&id, 0)
        .await
        .map_err(unavailable)?
        .iter()
        .any(|event| {
            event.kind == "preview_observation"
                && event
                    .payload
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    == Some(u64::from(request.version))
        });
    if observed {
        return Err(ApiError::Conflict(format!(
            "version {} of run '{id}' was observed already",
            request.version
        )));
    }
    let mut payload = serde_json::to_value(&request)
        .map_err(|err| ApiError::Internal(format!("the observation did not serialise: {err}")))?;
    payload["reportedBy"] = serde_json::json!(user.0.identity.username);
    publish_event(&state, &id, "preview_observation", payload).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The bounds of an observation (API/04 §5), or `None` when it keeps them.
fn observation_out_of_bounds(request: &PreviewObservationRequest) -> Option<String> {
    if request.version == 0 {
        return Some("version starts at 1".into());
    }
    if request.pages.is_empty() || request.pages.len() > MAX_OBSERVED_PAGES {
        return Some(format!("pages must hold 1 to {MAX_OBSERVED_PAGES} pages"));
    }
    for (index, page) in request.pages.iter().enumerate() {
        if page.label.chars().count() > MAX_PAGE_LABEL_CHARS {
            return Some(format!(
                "pages[{index}].label is longer than {MAX_PAGE_LABEL_CHARS} characters"
            ));
        }
        if page.text.chars().count() > MAX_PAGE_TEXT_CHARS {
            return Some(format!(
                "pages[{index}].text is longer than {MAX_PAGE_TEXT_CHARS} characters"
            ));
        }
        if page.rows.len() > MAX_OBSERVED_ENTRIES {
            return Some(format!(
                "pages[{index}].rows holds more than {MAX_OBSERVED_ENTRIES} counts"
            ));
        }
    }
    if request.failed_requests.len() > MAX_OBSERVED_ENTRIES {
        return Some(format!(
            "failedRequests holds more than {MAX_OBSERVED_ENTRIES} entries"
        ));
    }
    for (index, failed) in request.failed_requests.iter().enumerate() {
        if failed.path.chars().count() > MAX_PREVIEW_FILE_CHARS {
            return Some(format!(
                "failedRequests[{index}].path is longer than {MAX_PREVIEW_FILE_CHARS} characters"
            ));
        }
        if !(400..=599).contains(&failed.status) {
            return Some(format!(
                "failedRequests[{index}].status must be between 400 and 599"
            ));
        }
    }
    None
}

/// How long the Portal waits for `jc-functions`, whose own limit is 5 s of script.
const FUNCTION_TIMEOUT: Duration = Duration::from_secs(20);

/// How many runs this project may start in a day (PF-73, PF-74), asked by every door that
/// starts one: an application run and an assistant conversation (T-1403). The store answers
/// newest first, so the newest `max` runs are enough to know whether today is full.
pub(crate) async fn within_runs_per_day(state: &AppState, project: &str) -> Result<(), ApiError> {
    let Some(max) = crate::quotas::effective(&state.mirror, project).agent_runs_per_day else {
        return Ok(());
    };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let recent = state
        .agents
        .list_runs(project, i64::from(max))
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?;
    let started_today = recent
        .iter()
        .filter(|run| run.created_at.starts_with(&today))
        .count() as u32;
    if started_today >= max {
        return Err(crate::quotas::over(
            "agentRunsPerDay",
            started_today + 1,
            max,
            project,
        ));
    }
    Ok(())
}

/// `[a-z][a-z0-9-]{0,39}`: the name of a function file (Architecture/20 §3).
fn is_function_name(name: &str) -> bool {
    name.len() <= 40
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The access token the request was authenticated with: a bearer or the edge's. A Portal cookie
/// session keeps none, so its function calls reach the endpoint anonymously (API/04 §5).
fn caller_token(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let token = match Front::of(headers, state.config.trust_edge_token) {
        Front::Bearer => headers
            .get(header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?,
        Front::Edge => headers.get(&EDGE_TOKEN_HEADER)?.to_str().ok()?,
        Front::Portal => return None,
    };
    Some(token.to_owned())
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/functions/{fn}",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
        ("fn" = String, Path, description = "The function: `functions/{fn}.ts` of the run"),
    ),
    request_body(content = serde_json::Value, description = "The function's JSON body; an empty body is null", content_type = "application/json"),
    responses(
        (status = 200, description = "The function's own status and JSON body, whatever status it returned", body = serde_json::Value),
        (status = 400, description = "A body that is not JSON, or function files that do not build: every problem with file and line", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project, or no `functions/{fn}.ts` among its files", body = ProblemDetails),
        (status = 409, description = "The run is over", body = ProblemDetails),
        (status = 429, description = "The functions runtime is running all the calls it takes", body = ProblemDetails),
        (status = 500, description = "The function threw, ran out of memory or time, or returned more than 1 MiB: `{error: {message, file, line}}`", body = serde_json::Value),
        (status = 503, description = "No `JC_FUNCTIONS_URL`, no Keycloak client, a Portal built without the SDK server module, or a runtime that did not answer", body = ProblemDetails)
    )
)]
/// One call of a run's function in `jc-functions`: the run's current functions, the request and
/// the caller's own token, sent with the Portal's audience-bound token (SDK-18, SDK-23).
pub async fn call_function(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id, name)): Path<(String, String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let run = own_run_of(&state, &user, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and runs no function",
            run.status
        )));
    }
    let tool = format!("function:{name}");
    let input: serde_json::Value = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| ApiError::BadRequest(format!("the body is not JSON: {err}")))?
    };
    let invoked = invoke_function(
        &state,
        &run,
        &name,
        input.clone(),
        &query,
        &user.0.identity,
        caller_token(&state, &headers),
    )
    .await;
    let Invocation {
        outcome,
        duration_ms,
    } = match invoked {
        Ok(invocation) => invocation,
        Err(InvokeError::NoFunction) => {
            return Err(ApiError::NotFound(format!(
                "run '{id}' has no function '{name}'"
            )))
        }
        Err(InvokeError::DoesNotBuild(problems)) => {
            return Err(ApiError::Invalid {
                detail: format!(
                    "the run's functions do not build: {} problem(s)",
                    problems.len()
                ),
                errors: problems,
            })
        }
        Err(InvokeError::Unavailable(reason)) => return Err(ApiError::Unavailable(reason)),
        Err(InvokeError::Refused {
            reason,
            duration_ms,
            status,
        }) => {
            publish_event(
                &state,
                &id,
                "tool",
                serde_json::json!({
                    "tool": tool,
                    "status": "failed",
                    "durationMs": duration_ms,
                    "input": input,
                    "error": reason,
                }),
            )
            .await?;
            return Err(match status {
                RefusedStatus::NoAnswer => {
                    ApiError::Unavailable("jc-functions did not answer".into())
                }
                RefusedStatus::Full => ApiError::TooManyRequests(
                    "jc-functions is running all the calls it takes; try again".into(),
                ),
                RefusedStatus::TooLarge => ApiError::BadRequest(
                    "the function's body is larger than jc-functions takes (256 KiB)".into(),
                ),
                RefusedStatus::Other => ApiError::Unavailable(reason),
            });
        }
    };
    let status = outcome["status"]
        .as_u64()
        .and_then(|status| u16::try_from(status).ok())
        .and_then(|status| StatusCode::from_u16(status).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let logs = outcome
        .get("logs")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(error) = outcome.get("error").filter(|error| !error.is_null()) {
        // The runtime names files by their import name; the run knows them by path.
        let mut error = error.clone();
        if let Some(file) = error["file"].as_str() {
            error["file"] = serde_json::json!(file.strip_prefix(transpile::APP).unwrap_or(file));
        }
        publish_event(
            &state,
            &id,
            "tool",
            serde_json::json!({
                "tool": tool,
                "status": "failed",
                "durationMs": duration_ms,
                "input": input,
                "output": { "status": 500, "logs": logs },
                "error": error,
            }),
        )
        .await?;
        return Ok((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response());
    }
    publish_event(
        &state,
        &id,
        "tool",
        serde_json::json!({
            "tool": tool,
            "status": "ok",
            "durationMs": duration_ms,
            "input": input,
            "output": { "status": status.as_u16(), "logs": logs },
        }),
    )
    .await?;
    let body = outcome
        .get("body")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok((status, Json(body)).into_response())
}

/// What one invocation of a run's function returned: the runtime's outcome and how long it took.
pub(crate) struct Invocation {
    pub outcome: serde_json::Value,
    pub duration_ms: u64,
}

/// Why the runtime was not asked, or did not take the call.
pub(crate) enum InvokeError {
    NoFunction,
    DoesNotBuild(Vec<String>),
    /// This Portal cannot reach a runtime at all: no address, no client, no server module.
    Unavailable(String),
    /// The runtime did not take the call; `reason` is what the run's log says.
    Refused {
        reason: String,
        duration_ms: u64,
        status: RefusedStatus,
    },
}

pub(crate) enum RefusedStatus {
    NoAnswer,
    Full,
    TooLarge,
    Other,
}

/// One call of a run's function in `jc-functions`, as `identity` (SDK-18, SDK-23): the route
/// and the editing agent's `call_function` tool share it (SDK-20). `caller_token` is the
/// person's own edge token when the call comes through the edge, nothing otherwise.
pub(crate) async fn invoke_function(
    state: &AppState,
    run: &AgentRun,
    name: &str,
    input: serde_json::Value,
    query: &BTreeMap<String, String>,
    identity: &crate::auth::session::Identity,
    caller_token: Option<String>,
) -> Result<Invocation, InvokeError> {
    let files: BTreeMap<String, String> = run
        .files
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(path, _)| path.starts_with("functions/"))
        .filter_map(|(path, text)| Some((path.clone(), text.as_str()?.to_owned())))
        .collect();
    let entry = format!("functions/{name}.ts");
    if !is_function_name(name) || !files.contains_key(&entry) {
        return Err(InvokeError::NoFunction);
    }
    let built = transpile::transpile(&files);
    let problems: Vec<String> = built
        .problems
        .iter()
        .filter(|problem| !problem.file.contains(".test."))
        .map(ToString::to_string)
        .collect();
    if !problems.is_empty() {
        return Err(InvokeError::DoesNotBuild(problems));
    }
    let runtime = state.config.functions_url.as_deref().ok_or_else(|| {
        InvokeError::Unavailable(
            "this Portal has no jc-functions address (JC_FUNCTIONS_URL)".into(),
        )
    })?;
    let oidc = state.oidc.as_ref().ok_or_else(|| {
        InvokeError::Unavailable(
            "this Portal has no Keycloak client to authenticate to jc-functions with".into(),
        )
    })?;
    let server = kit::functions_server().ok_or_else(|| {
        InvokeError::Unavailable(
            "this Portal was built without the SDK server module (sdk/dist/functions-server.js)"
                .into(),
        )
    })?;
    let service = oidc
        .service_token()
        .await
        .map_err(|err| InvokeError::Unavailable(format!("no token for jc-functions: {err}")))?;

    let space = state
        .mirror
        .get(&run.project, "Endpoint", &run.endpoint_name)
        .and_then(|env| crate::api::assistant::ref_name(&env.spec["contextSpaceRef"]))
        .unwrap_or_else(|| run.project.clone());
    let mut modules = built.functions;
    modules.insert("@joinedcontext/sdk/server".to_owned(), server);
    let mut invocation = serde_json::json!({
        "files": modules,
        "entry": format!("{}{entry}", transpile::APP),
        "request": {
            "method": "POST",
            "query": query,
            "body": input,
            "user": {
                "id": identity.subject,
                "name": identity.name.clone().unwrap_or_else(|| identity.username.clone()),
                "email": identity.email,
                "roles": identity.roles,
            },
        },
        "config": {
            "slug": run.endpoint_slug,
            "orgDomain": crate::api::assistant::org_domain(state, &run.project),
            "space": space,
        },
        "token": caller_token,
    });
    let run_endpoints = crate::agents::endpoints::of_run(run);
    if run_endpoints.len() > 1 {
        invocation["config"]["endpoints"] =
            crate::agents::endpoints::config(&run_endpoints, &run.data_needs);
    }

    let started = std::time::Instant::now();
    let answer = functions_http()
        .post(format!("{runtime}/invoke"))
        .bearer_auth(service)
        .json(&invocation)
        .send()
        .await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let refused = |reason: String, status: RefusedStatus| InvokeError::Refused {
        reason,
        duration_ms,
        status,
    };
    let response = match answer {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(error = %err, "jc-functions did not answer");
            return Err(refused(
                "jc-functions did not answer".into(),
                RefusedStatus::NoAnswer,
            ));
        }
    };
    match response.status() {
        StatusCode::OK => {}
        StatusCode::TOO_MANY_REQUESTS => {
            return Err(refused("jc-functions is full".into(), RefusedStatus::Full))
        }
        StatusCode::PAYLOAD_TOO_LARGE => {
            return Err(refused(
                "the request is too large".into(),
                RefusedStatus::TooLarge,
            ))
        }
        status => {
            tracing::warn!(%status, "jc-functions refused an invocation");
            return Err(refused(
                format!("jc-functions answered {status}"),
                RefusedStatus::Other,
            ));
        }
    }
    let outcome: serde_json::Value = response.json().await.map_err(|err| {
        refused(
            format!("jc-functions answered no outcome: {err}"),
            RefusedStatus::Other,
        )
    })?;
    Ok(Invocation {
        outcome,
        duration_ms,
    })
}

/// One client for every invocation, without redirects: the runtime's address is configuration.
fn functions_http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(FUNCTION_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default()
    })
}

pub(crate) async fn end_run(
    state: &AppState,
    run: &AgentRun,
    status: AgentRunStatus,
    reason: &str,
) -> Result<AgentRun, ApiError> {
    let ended = state
        .agents
        .set_status(&run.id, status, Some(reason))
        .await
        .map_err(status_error)?;

    // The ticket first, the pod second: between the two calls the workspace is already
    // refused by the proxy, where the other order would leave a live credential for a moment
    // after the user asked for it to stop (AG-46).
    state
        .agents
        .invalidate_ticket(&run.id)
        .await
        .map_err(unavailable)?;
    if let Some(settings) = state.config.agent_settings.as_ref() {
        kube::delete_workspace_job(state.kube.as_deref(), &settings.namespace, &run.id).await;
    }
    publish_event(state, &run.id, "status", status_payload(status)).await?;
    Ok(ended)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/cancel",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    responses(
        (status = 200, description = "The cancelled run", body = AgentRun),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run is already over", body = ProblemDetails)
    )
)]
pub async fn cancel_run(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<AgentRun>, ApiError> {
    let run = own_run_of(&state, &user, &project, &id).await?;
    crate::permissions::for_request(&state, &user.0.identity, &project).check(
        "App",
        jc_core::kinds::Verb::Propose,
        None,
    )?;

    let reason = format!("cancelled by {}", user.0.identity.username);
    let cancelled = end_run(&state, &run, AgentRunStatus::Cancelled, &reason).await?;
    Ok(Json(cancelled))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/agent-runs/{id}/publish",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run id"),
    ),
    responses(
        (status = 202, description = "The Change that publishes the application", body = crate::change::Change),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run in this project", body = ProblemDetails),
        (status = 409, description = "The run has nothing to publish yet", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails)
    )
)]
pub async fn publish_run(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let run = own_run_of(&state, &user, &project, &id).await?;
    if run.kind == "dashboard" {
        return Err(ApiError::Conflict(
            "publishing a dashboard run is not available yet".into(),
        ));
    }
    if run.kind == "analysis" {
        return Err(ApiError::Conflict("an analysis is never published".into()));
    }
    let status = AgentRunStatus::parse(&run.status);
    // An unattended run ends waiting for approval with its preview built, so it is published
    // from there (AG-69).
    let waiting = run.unattended && status == Some(AgentRunStatus::AwaitingApproval);
    if status != Some(AgentRunStatus::Previewing) && !waiting {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}'; only a run that has a preview is published (AP-46)",
            run.status
        )));
    }

    // The application is published the way every other manifest is: one merge request, the
    // project's own approval rules, no path around review (AP-10, AP-55).
    let manifest = app_manifest(&run);
    let operation = match state.mirror.get(&project, "App", &run.app_name) {
        Some(_) => crate::change::Operation::Update,
        None => crate::change::Operation::Create,
    };
    let outcome = crate::api::mutate::propose_with_identity(
        &user.0.identity,
        &state,
        &project,
        "apps",
        Some(&run.app_name),
        operation,
        false,
        manifest,
    )
    .await?;
    let crate::api::mutate::ProposeOutcome::Change(change) = outcome else {
        return Err(ApiError::Internal(
            "publishing proposed a dry run instead of a Change".into(),
        ));
    };
    // The run remembers its Change, so its page shows it and approves it in place (AP-71).
    if let Ok(number) = crate::api::changes::parse_change_id(&change.metadata.name) {
        if let Ok(number) = i32::try_from(number) {
            state
                .agents
                .set_merge_request(&id, number)
                .await
                .map_err(unavailable)?;
        }
    }
    let response = (StatusCode::ACCEPTED, Json(change)).into_response();

    if !waiting {
        state
            .agents
            .set_status(&id, AgentRunStatus::AwaitingApproval, None)
            .await
            .map_err(status_error)?;
        publish_event(
            &state,
            &id,
            "status",
            status_payload(AgentRunStatus::AwaitingApproval),
        )
        .await?;
    }
    Ok(response)
}

/// The `App` manifest a published run leaves behind (AP-01, AP-51).
///
/// Built from the run rather than from anything the workspace wrote: the agent's commits are the
/// application's source, and what the platform deploys is derived from the request a person
/// approved. `dataNeeds` is the list that was checked against the endpoint before the run
/// started, so the manifest cannot widen what the application may reach.
fn app_manifest(run: &AgentRun) -> serde_json::Value {
    serde_json::json!({
        "apiVersion": crate::resource::API_VERSION,
        "kind": "App",
        "metadata": {
            "name": run.app_name,
            "namespace": run.project,
            "annotations": {
                // What a reviewer of the merge request needs to find the conversation the
                // application came out of, and to see that the prompt has not been edited since.
                AGENT_RUN_ANNOTATION: run.id,
                PROMPT_DIGEST_ANNOTATION: run.prompt_digest,
            },
        },
        "spec": {
            "kind": run.app_class,
            // Beside the manifest, which is where the agent committed it (`path_prefix`).
            "source": { "path": "./src" },
            "build": build_toolchains(&run.app_class),
            "visibility": run.visibility,
            "lifecycle": "published",
            "dataNeeds": run.data_needs,
        },
    })
}

/// The toolchains CI pins for a generated application (AP-11). A `static` application is a Vite
/// build and nothing else; the other two classes carry a Rust binary with the build embedded.
fn build_toolchains(app_class: &str) -> serde_json::Value {
    if app_class == "static" {
        serde_json::json!({ "node": NODE_TOOLCHAIN })
    } else {
        serde_json::json!({ "rust": RUST_TOOLCHAIN, "node": NODE_TOOLCHAIN })
    }
}

// ---------------------------------------------------------------------------------------------
// The internal listener (AG-52). No session, no CSRF, one bearer: the proxy's own token.
// ---------------------------------------------------------------------------------------------

pub async fn internal_get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RunContext>, ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' not found")))?;
    let profile = Profile::load(&state.mirror, &run.profile)?;
    let endpoint_slugs = crate::agents::endpoints::of_run(&run)
        .into_iter()
        .map(|endpoint| endpoint.slug)
        .collect();
    Ok(Json(RunContext {
        id: run.id,
        project: run.project,
        app_name: run.app_name,
        endpoint_slug: run.endpoint_slug,
        endpoint_slugs,
        allows_write: run.allows_write,
        branch: run.branch,
        path_prefix: run.path_prefix,
        status: run.status,
        ticket_hash: run.ticket_hash,
        max_tokens: profile.max_tokens_per_run,
        allowed_hosts: profile.allowed_hosts,
        requests_per_minute: profile.requests_per_minute,
        steps_per_run: profile.steps_per_run,
        max_response_bytes: profile.max_response_bytes,
        max_egress_bytes_per_run: profile.max_egress_bytes_per_run,
        created_by: run.created_by,
        model_name: profile.model_name,
        reasoning_effort: profile.reasoning_effort,
    }))
}

/// What a run may read about a resource of its own project when a step failed (AG-57).
///
/// The proxy has already refused an unknown component and an id that is not a name; here the
/// run's project is the only project asked, so a run learns nothing about another project's
/// pipelines or changes, not even that they exist.
pub async fn internal_diagnostics(
    State(state): State<AppState>,
    Path((id, component, name)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' not found")))?;
    let body = match component.as_str() {
        "pipeline" => serde_json::to_value(
            crate::api::pipelines::metrics_for(&state, &run.project, &name).await?,
        ),
        "change" => serde_json::to_value(
            crate::api::changes::change_for(&state, &run.project, &name).await?,
        ),
        other => {
            return Err(ApiError::BadRequest(format!(
                "the diagnostics door knows no component '{other}'"
            )))
        }
    }
    .map_err(|err| ApiError::Internal(err.to_string()))?;
    Ok(Json(body))
}

/// What the person said, for the workspace to act on (AG-45, AG-52).
///
/// One call, one channel: an agent that wants to know whether a question was answered and
/// whether it was told to change course asks here and nowhere else. The call waits rather than
/// answering empty immediately, because the alternative is a workspace polling in a loop and
/// spending its request budget on nothing.
pub async fn internal_inbox(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<InboxQuery>,
    headers: HeaderMap,
) -> Result<Json<Inbox>, ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' not found")))?;

    // Subscribed before the store is read, so an event that lands between the two is waited
    // for rather than missed.
    let mut live = state.agent_events.subscribe(&run.id).await;
    let items = inbox_items(&state, &run.id, query.after).await?;
    if !items.is_empty() || terminal(&run) {
        return Ok(Json(Inbox { items }));
    }

    let wait = query.wait.unwrap_or(INBOX_WAIT_SECS).min(INBOX_WAIT_SECS);
    if wait == 0 {
        return Ok(Json(Inbox { items }));
    }
    let _ = tokio::time::timeout(Duration::from_secs(wait), async {
        loop {
            match live.recv().await {
                Ok(event)
                    if event.seq > query.after && INBOX_KINDS.contains(&event.kind.as_str()) =>
                {
                    return;
                }
                // A lagged receiver missed something; the store below is the truth either way.
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
    .await;

    Ok(Json(Inbox {
        items: inbox_items(&state, &run.id, query.after).await?,
    }))
}

async fn inbox_items(
    state: &AppState,
    run_id: &str,
    after: i64,
) -> Result<Vec<RelayedInbound>, ApiError> {
    Ok(state
        .agents
        .events_since(run_id, after)
        .await
        .map_err(unavailable)?
        .into_iter()
        .filter(|event| INBOX_KINDS.contains(&event.kind.as_str()))
        .map(|event| RelayedInbound {
            seq: event.seq,
            kind: event.kind,
            payload: event.payload,
        })
        .collect())
}

/// The body arrives as bytes and is parsed after the caller is known (T-2271): an extractor runs
/// before the handler does, so `Json<RelayedEvent>` answered 422 to a call carrying no identity at
/// all, which tells whoever reaches the port what shape the route wants.
pub async fn internal_post_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<EventReceipt>), ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let relayed: RelayedEvent = serde_json::from_slice(&body)
        .map_err(|error| ApiError::BadRequest(format!("this is not a relayed event: {error}")))?;
    let run = state
        .agents
        .get_run(&relayed.run_id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{}' not found", relayed.run_id)))?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{}' is '{}' and takes no more events",
            run.id, run.status
        )));
    }

    // A navigate event is checked before it is recorded: a route that is not a path inside the
    // Portal never reaches the log, let alone a browser (UI-45).
    if relayed.kind == "navigate" {
        navigate_route(&relayed.payload)?;
    }
    // An event is a record first. The three kinds that also move something are applied after
    // it is recorded, so a stream never shows a state the log does not explain.
    let event = publish_event(&state, &run.id, &relayed.kind, relayed.payload.clone()).await?;

    match relayed.kind.as_str() {
        "usage" => {
            let tokens = relayed
                .payload
                .get("tokensThisStep")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            state
                .agents
                .record_usage(&run.id, tokens, 1)
                .await
                .map_err(unavailable)?;
        }
        "preview" => {
            if let Some(url) = relayed
                .payload
                .get("previewUrl")
                .and_then(serde_json::Value::as_str)
            {
                state
                    .agents
                    .set_preview_url(&run.id, url)
                    .await
                    .map_err(unavailable)?;
                // The first frame is counted once: `set_preview_url` keeps the first value.
                if run.first_frame_ms.is_none() {
                    if let Ok(Some(updated)) = state.agents.get_run(&run.id).await {
                        if let Some(ms) = updated.first_frame_ms {
                            crate::telemetry::record_run_timing(
                                "first_frame",
                                &updated.profile,
                                ms,
                            );
                        }
                    }
                }
            }
        }
        "status" => {
            let next = relayed
                .payload
                .get("status")
                .and_then(serde_json::Value::as_str)
                .and_then(AgentRunStatus::parse)
                .ok_or_else(|| {
                    ApiError::BadRequest("a status event names no state of a run".into())
                })?;
            let error = relayed
                .payload
                .get("error")
                .and_then(serde_json::Value::as_str);
            state
                .agents
                .set_status(&run.id, next, error)
                .await
                .map_err(status_error)?;
        }
        _ => {}
    }

    Ok((StatusCode::CREATED, Json(EventReceipt { seq: event.seq })))
}

/// The longest route a `navigate` event may name (UI-45).
const MAX_ROUTE_CHARS: usize = 512;

/// The route of a `navigate` event, or why it is refused (UI-45, API/04 §4).
///
/// Only a path inside the Portal passes: one leading `/`, no scheme, no `//` (a
/// protocol-relative URL), no `#`, no control character, at most 512 characters. The prefill,
/// when present, is an object; its values are the form's problem, not this gate's.
fn navigate_route(payload: &serde_json::Value) -> Result<&str, ApiError> {
    let route = payload
        .get("route")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("a navigate event names no route".into()))?;
    let inside = route.starts_with('/')
        && !route.starts_with("//")
        && route.len() <= MAX_ROUTE_CHARS
        && !route.contains('#')
        && !route.contains(':')
        && !route.chars().any(char::is_control);
    if !inside {
        return Err(ApiError::BadRequest(
            "a navigate route must be a path inside the Portal".into(),
        ));
    }
    match payload.get("prefill") {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Object(_)) => Ok(route),
        Some(_) => Err(ApiError::BadRequest(
            "a navigate prefill must be an object".into(),
        )),
    }
}

/// The credential proxy's own ServiceAccount token, audience-bound to this listener and matched by
/// the client it was issued to (AG-52, T-2271).
///
/// It was a string both sides held, read from `JC_AGENT_PROXY_TOKEN` and compared in constant time.
/// Constant time was the least of it: a shared secret between two services never rotates, appears in
/// two configurations, and gives whoever reads either of them every callback of every run. The token
/// is minted per proxy from the realm now and expires by itself.
async fn authenticate_proxy(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    crate::auth::internal::authenticate_agent_proxy(state, headers).await
}

// ---------------------------------------------------------------------------------------------

/// The run, if it is this project's. A run of another project is a 404 rather than a 403: the
/// caller has no business learning that the id exists.
async fn run_of(state: &AppState, project: &str, id: &str) -> Result<AgentRun, ApiError> {
    match state.agents.get_run(id).await.map_err(unavailable)? {
        Some(run) if run.project == project => Ok(run),
        _ => Err(ApiError::NotFound(format!(
            "run '{id}' not found in project '{project}'"
        ))),
    }
}

/// The run of a caller who may see it: the person who started it, or a caller who may approve in
/// the project — the rule `list_runs` already applies to the listing (AG-43, AG-45, PF-50). A run
/// keeps the identity of whoever created it and acts with their grants, so a second person who
/// learns the id (a shared link, a published App's annotation) must not read its conversation or
/// send it an instruction. Refused as `404`, like a run of another project: whether the id exists
/// is not the caller's business (T-0801).
async fn own_run_of(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    id: &str,
) -> Result<AgentRun, ApiError> {
    let run = run_of(state, project, id).await?;
    if run.created_by == user.0.identity.username
        || crate::api::changes::may_approve_anything(state, &user.0.identity, project).is_ok()
    {
        return Ok(run);
    }
    Err(ApiError::NotFound(format!(
        "run '{id}' not found in project '{project}'"
    )))
}

/// Records one event and hands it to every connected stream.
pub(crate) async fn publish_event(
    state: &AppState,
    run_id: &str,
    kind: &str,
    payload: serde_json::Value,
) -> Result<AgentRunEvent, ApiError> {
    let event = state
        .agents
        .append_event(run_id, kind, payload)
        .await
        .map_err(unavailable)?;
    state.agent_events.broadcast(&event).await;
    Ok(event)
}

fn sse(event: &AgentRunEvent) -> Event {
    let mut data = event.payload.clone();
    // The sequence number is on the frame as its id and in the body, because the API examples
    // show a client reading `seq` out of the payload without parsing the frame (API/04 §4).
    if let Some(object) = data.as_object_mut() {
        object.insert("seq".to_owned(), serde_json::json!(event.seq));
    }
    Event::default()
        .id(event.seq.to_string())
        .event(event.kind.clone())
        .data(data.to_string())
}

pub(crate) fn status_payload(status: AgentRunStatus) -> serde_json::Value {
    serde_json::json!({ "status": status.as_str(), "timestamp": now_rfc3339() })
}

fn terminal(run: &AgentRun) -> bool {
    AgentRunStatus::parse(&run.status).is_some_and(|status| status.is_terminal())
}

pub(crate) fn agent_settings(state: &AppState) -> Result<&AgentSettings, ApiError> {
    state.config.agent_settings.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no agent runner configured (AG-33)".into())
    })
}

pub(crate) fn expiry(ttl_secs: i64) -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::seconds(ttl_secs))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

pub(crate) fn unavailable(err: StoreError) -> ApiError {
    ApiError::Unavailable(err.to_string())
}

fn status_error(err: StatusChangeError) -> ApiError {
    match err {
        StatusChangeError::Unknown => ApiError::NotFound("no such run".into()),
        StatusChangeError::Refused { .. } => ApiError::Conflict(err.to_string()),
        StatusChangeError::Store(store) => unavailable(store),
    }
}

/// The project routes, under `/api/v1`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/agent-runs",
            get(list_runs).post(create_run),
        )
        .route("/projects/{project}/agent-runs/{id}", get(get_run))
        .route(
            "/projects/{project}/agent-runs/{id}/events",
            get(stream_events),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/answers",
            post(answer_question),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/messages",
            post(post_message),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/preview-errors",
            post(post_preview_error),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/preview-observations",
            post(post_preview_observation),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/functions/{fn}",
            post(call_function),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/cancel",
            post(cancel_run),
        )
        .route(
            "/projects/{project}/agent-runs/{id}/publish",
            post(publish_run),
        )
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/agent-runs/{id}/preview",
    tag = "agents",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "Run identifier"),
    ),
    responses(
        (status = 200, description = "One document: a code run's interface on the SDK runtime, or the kit rendering a kit run's specification", content_type = "text/html"),
        (status = 400, description = "A code run's files do not build: every problem with file and line", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run, or no pass has written files yet", body = ProblemDetails),
        (status = 503, description = "This Portal was built without the kit or the SDK runtime", body = ProblemDetails)
    )
)]
/// The preview of a run in one document, because the frame it is shown in has no origin to
/// fetch anything else with (AP-50, AP-60, UI-41): a code run's `src/**` transpiled onto the SDK
/// runtime (SDK-16), else the kit bundle rendering `spec.json`.
pub async fn preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let run = own_run_of(&state, &user, &project, &id).await?;
    let code: BTreeMap<String, String> = run
        .files
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(path, _)| path.starts_with("src/") || path.starts_with("functions/"))
        .filter_map(|(path, text)| Some((path.clone(), text.as_str()?.to_owned())))
        .collect();
    if !code.is_empty() {
        return code_preview(&state, &run, &code, &headers);
    }
    let text = run
        .files
        .get(kit::SPEC_FILE)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' has no preview yet")))?;
    let spec = kit::parse(text).map_err(|errors| {
        ApiError::Internal(format!(
            "the stored spec.json does not validate: {}",
            errors.join("; ")
        ))
    })?;
    let data = run
        .files
        .get(kit::DATA_FILE)
        .and_then(serde_json::Value::as_str)
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok());
    // A page the model wrote replaces the kit (the escape hatch); an empty one hands back.
    let page = run
        .files
        .get(kit::PAGE_FILE)
        .and_then(serde_json::Value::as_str)
        .filter(|html| !html.trim().is_empty());
    let origin = {
        let url = &state.config.public_base_url;
        match url.port() {
            Some(port) => format!(
                "{}://{}:{port}",
                url.scheme(),
                url.host_str().unwrap_or_default()
            ),
            None => format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default()),
        }
    };
    // The field schema of AP-61: what the form's inputs are, from the space's DataModel.
    let types: Vec<String> = spec.sources.iter().map(|s| s.entity_type.clone()).collect();
    let schema =
        crate::agents::fields::for_endpoint(&state, &project, &run.endpoint_slug, &types).await;
    let basemap_url = crate::api::basemap::style_url(&state.config, &project);
    let (html, csp) = match page {
        Some(page) => (
            kit::page_document(
                page,
                &run.endpoint_slug,
                &spec,
                data.as_ref(),
                schema.as_ref(),
                basemap_url.as_deref(),
            ),
            kit::page_content_security_policy(&origin),
        ),
        None => {
            let bundle = kit::bundle().ok_or_else(|| {
                ApiError::Unavailable(
                    "this Portal was built without the kit (sdk/dist is empty)".into(),
                )
            })?;
            (
                kit::document(
                    &spec.title,
                    &run.endpoint_slug,
                    &spec,
                    data.as_ref(),
                    schema.as_ref(),
                    &bundle,
                    basemap_url.as_deref(),
                ),
                kit::content_security_policy(&origin, &kit::script_hash(&bundle.js)),
            )
        }
    };
    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp),
            (header::X_FRAME_OPTIONS, "SAMEORIGIN".to_owned()),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        html,
    )
        .into_response())
}

/// One rendered preview: its ETag, and the policy and document it answers with.
type RenderedPreview = (String, std::sync::Arc<(String, String)>);

/// Rendered code previews by the digest of what they are made of, newest last: reopening a run
/// serves the document it already rendered instead of transpiling every file again.
static RENDERED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::VecDeque<RenderedPreview>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::VecDeque::new()));

/// How many rendered previews the Portal keeps.
const RENDERED_KEEP: usize = 16;

/// A code run's document: the SDK configured for the bridge on the run's endpoint, the
/// basemap route the one address it may connect to (SDK-16, AP-63, AP-67).
fn code_preview(
    state: &AppState,
    run: &AgentRun,
    files: &BTreeMap<String, String>,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let space = state
        .mirror
        .get(&run.project, "Endpoint", &run.endpoint_name)
        .and_then(|env| crate::api::assistant::ref_name(&env.spec["contextSpaceRef"]))
        .unwrap_or_else(|| run.project.clone());
    let mut config = serde_json::json!({
        "slug": run.endpoint_slug,
        "orgDomain": crate::api::assistant::org_domain(state, &run.project),
        "space": space,
        "transport": "bridge",
        "appName": run.app_name,
        "endpointName": run.endpoint_name,
    });
    let run_endpoints = crate::agents::endpoints::of_run(run);
    if run_endpoints.len() > 1 {
        config["endpoints"] = crate::agents::endpoints::config(&run_endpoints, &run.data_needs);
    }
    if let Some(url) = crate::api::basemap::style_url(&state.config, &run.project) {
        config["basemap"] = serde_json::Value::String(url);
    }
    let route = crate::api::basemap::route_prefix(&state.config, &run.project);
    // The document is a function of the files, the configuration and the route: its digest is
    // the ETag, so a browser reopening an unchanged version gets a 304 and the Portal a hit.
    let etag = {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        for (path, content) in files {
            hash.update(path.as_bytes());
            hash.update([0]);
            hash.update(content.as_bytes());
            hash.update([0]);
        }
        hash.update(config.to_string().as_bytes());
        hash.update(route.as_deref().unwrap_or_default().as_bytes());
        hash.update(env!("CARGO_PKG_VERSION").as_bytes());
        format!("\"{:x}\"", hash.finalize())
    };
    let cached = RENDERED.lock().ok().and_then(|kept| {
        kept.iter()
            .find(|(key, _)| *key == etag)
            .map(|(_, doc)| doc.clone())
    });
    if cached.is_some()
        && headers
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(',').any(|tag| tag.trim() == etag))
    {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, "private, no-cache".to_owned()),
            ],
        )
            .into_response());
    }
    if let Some(doc) = cached {
        return Ok(preview_response(&etag, &doc.1, doc.0.clone()));
    }
    let document =
        preview::document(files, &run.app_name, &config, route.as_deref()).map_err(|refusal| {
            match refusal {
                preview::Refusal::NoRuntime => ApiError::Unavailable(
                    "this Portal was built without the SDK runtime (sdk/dist/runtime is empty)"
                        .into(),
                ),
                preview::Refusal::Problems(problems) => ApiError::Invalid {
                    detail: format!(
                        "the run's files do not build a preview: {} problem(s)",
                        problems.len()
                    ),
                    errors: problems.iter().map(ToString::to_string).collect(),
                },
            }
        })?;
    if let Ok(mut kept) = RENDERED.lock() {
        kept.push_back((
            etag.clone(),
            std::sync::Arc::new((document.html.clone(), document.csp.clone())),
        ));
        while kept.len() > RENDERED_KEEP {
            kept.pop_front();
        }
    }
    Ok(preview_response(&etag, &document.csp, document.html))
}

/// A rendered preview: the page, its script policy, and a validator instead of `no-store`.
fn preview_response(etag: &str, csp: &str, html: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (header::CONTENT_SECURITY_POLICY, csp.to_owned()),
            (header::X_FRAME_OPTIONS, "SAMEORIGIN".to_owned()),
            (header::CACHE_CONTROL, "private, no-cache".to_owned()),
            (header::ETAG, etag.to_owned()),
        ],
        html,
    )
        .into_response()
}

/// The preview route alone, merged beside the Portal's own routes rather than under them: the
/// document carries its own policy, and the Portal's `script-src 'self'` would override it.
pub fn preview_router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/projects/{project}/agent-runs/{id}/preview",
        get(preview),
    )
}

/// What the Portal accepts on one relayed event: the proxy's 64 KiB ceiling
/// (`Architecture/19 §4`) plus the envelope the proxy wraps it in — the run id and the kind.
///
/// The proxy already refuses a larger event, and this is the same ceiling on the door behind it,
/// so a caller that reaches the internal listener another way is held to what the documented
/// route promises rather than to axum's default (AG-45, AG-46).
const MAX_RELAYED_EVENT_BYTES: usize = 64 * 1024 + 1024;

/// The operations registry, reached by an agent run through the proxy (AG-64, AG-70).
///
/// One registry behind every door: this is the same dispatcher a person's MCP client speaks to,
/// entered as the person who started the run and narrowed by the run's profile. The two halves
/// are the point — a profile may take away and never add, and `Via::Agent` keeps AG-11 whatever
/// the profile says, so no agent approves a change through this door either.
pub async fn internal_mcp(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::response::Response, ApiError> {
    authenticate_proxy(&state, &headers).await?;
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' not found")))?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{}' is '{}' and calls nothing more",
            run.id, run.status
        )));
    }
    let identity: crate::auth::session::Identity = serde_json::from_value(run.starter.clone())
        .map_err(|_| {
            ApiError::Denied(format!(
                "run '{}' carries no starter, so there is nobody to run its calls as",
                run.id
            ))
        })?;
    let profile = Profile::load(&state.mirror, &run.profile)?;
    let caller = crate::ops::Caller::for_run(identity, profile.access.clone());
    Ok(crate::mcp::dispatch_for(state, caller, body).await)
}

/// The two routes the credential proxy calls, served on the internal listener alone (AG-52).
pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/internal/agent-runs/{id}/mcp", post(internal_mcp))
        .route(
            "/internal/agent-runs/events",
            post(internal_post_event).layer(axum::extract::DefaultBodyLimit::max(
                MAX_RELAYED_EVENT_BYTES,
            )),
        )
        .route("/internal/agent-runs/{id}", get(internal_get_run))
        .route("/internal/agent-runs/{id}/inbox", get(internal_inbox))
        .route(
            "/internal/agent-runs/{id}/diagnostics/{component}/{name}",
            get(internal_diagnostics),
        )
}

#[cfg(test)]
mod tests {
    use super::{caller_token, is_function_name, navigate_route};
    use axum::http::{header, HeaderMap, HeaderValue};
    use serde_json::json;

    #[test]
    fn a_function_name_is_lowercase_letters_digits_and_dashes() {
        for name in ["summary", "a", "count-by-type-2", &"a".repeat(40)] {
            assert!(is_function_name(name), "{name}");
        }
        for name in [
            "",
            "Summary",
            "2x",
            "-x",
            "a_b",
            "a.b",
            "..",
            &"a".repeat(41),
        ] {
            assert!(!is_function_name(name), "{name}");
        }
    }

    #[test]
    fn the_callers_token_is_the_bearer_or_the_trusted_edge_header_never_a_cookie() {
        let mut config = crate::config::Config::for_tests();
        let edge = |headers: &[(&str, &str)]| {
            let mut map = HeaderMap::new();
            for (name, value) in headers {
                map.insert(
                    header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                    HeaderValue::from_str(value).unwrap(),
                );
            }
            map
        };
        let state = crate::state::AppState::new(config.clone(), None);
        assert_eq!(
            caller_token(&state, &edge(&[("authorization", "Bearer abc")])).as_deref(),
            Some("abc")
        );
        assert_eq!(
            caller_token(&state, &edge(&[("x-access-token", "edge")])),
            None
        );
        assert_eq!(
            caller_token(&state, &edge(&[("cookie", "jc_session=x")])),
            None
        );
        assert_eq!(
            caller_token(&state, &edge(&[("authorization", "Basic abc")])),
            None
        );
        config.trust_edge_token = true;
        let state = crate::state::AppState::new(config, None);
        assert_eq!(
            caller_token(&state, &edge(&[("x-access-token", "edge")])).as_deref(),
            Some("edge")
        );
    }

    #[test]
    fn a_portal_path_with_a_prefill_passes() {
        let payload = json!({
            "route": "/projects/helsinki/endpoints?tab=all",
            "prefill": {"name": "air-quality-public"}
        });
        assert_eq!(
            navigate_route(&payload).unwrap(),
            "/projects/helsinki/endpoints?tab=all"
        );
        assert!(navigate_route(&json!({"route": "/"})).is_ok());
        assert!(navigate_route(&json!({"route": "/x", "prefill": null})).is_ok());
    }

    #[test]
    fn anything_that_is_not_a_path_inside_the_portal_is_refused() {
        for route in [
            "https://evil.example/",
            "//evil.example/projects",
            "javascript:alert(1)",
            "projects/helsinki",
            "/projects/helsinki#/x",
            "/projects/hel\nsinki",
            "",
        ] {
            assert!(
                navigate_route(&json!({ "route": route })).is_err(),
                "{route:?}"
            );
        }
        let long = format!("/{}", "a".repeat(512));
        assert!(navigate_route(&json!({ "route": long })).is_err());
        assert!(navigate_route(&json!({ "prefill": {} })).is_err());
        assert!(navigate_route(&json!({ "route": "/x", "prefill": "name=x" })).is_err());
        assert!(navigate_route(&json!({ "route": "/x", "prefill": [1] })).is_err());
    }
}
