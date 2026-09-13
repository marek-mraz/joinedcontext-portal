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
use utoipa::ToSchema;

use crate::agents::kube;
use crate::agents::needs::validate_data_needs;
use crate::agents::profile::Profile;
use crate::agents::run::{
    digest_prompt, mint_run_id, mint_ticket, AgentRun, AgentRunEvent, AgentRunStatus,
};
use crate::agents::store::{now_rfc3339, StatusChangeError, StoreError};
use crate::agents::{kit, oneshot};
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
const MAX_PROMPT_CHARS: usize = 4_000;
/// Longest instruction a person may send into a live run. Same ceiling as the prompt: it is a
/// sentence of steering, and a workspace reads it with the same budget as everything else.
const MAX_MESSAGE_CHARS: usize = 4_000;
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
    /// The `Endpoint` the application reads through. Nothing else is reachable.
    pub endpoint_name: String,
    /// Which `AgentProfile` runs. Defaults to the builder profile the platform ships.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// `static`, `service` or `fullstack`, as the `App` kind spells them.
    pub app_class: String,
    /// Who may reach the published application. `public` is refused (AP-42).
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// What the application should do, in the person's own words.
    pub prompt: String,
    /// The types, attributes and operations the application needs. Checked against what the
    /// endpoint publishes before anything is scheduled (AP-44).
    pub data_needs: Vec<serde_json::Value>,
}

fn default_profile() -> String {
    "app-builder".to_owned()
}

fn default_visibility() -> String {
    "project".to_owned()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    pub limit: Option<i64>,
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
    pub allows_write: bool,
    pub branch: String,
    pub path_prefix: String,
    pub status: String,
    pub ticket_hash: String,
    pub max_tokens: u64,
    pub allowed_hosts: Vec<String>,
    pub requests_per_minute: u32,
    pub max_response_bytes: u64,
    pub created_by: String,
    pub model_name: String,
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

    // AP-42 and AP-44 in one place: public is refused, and every violation of what the
    // endpoint publishes is named at once.
    let allows_write = validate_data_needs(
        &state.mirror,
        &project,
        &request.endpoint_name,
        &request.visibility,
        &request.data_needs,
        &user,
    )?;

    let endpoint_slug = endpoint_slug(&state, &project, &request.endpoint_name)?;

    let id = mint_run_id();
    let (ticket, ticket_hash) = mint_ticket();
    let created_at = now_rfc3339();
    let expires_at = expiry(settings.run_ttl_secs);
    let run = AgentRun {
        id: id.clone(),
        project: project.clone(),
        app_name: request.app_name.clone(),
        endpoint_name: request.endpoint_name.clone(),
        endpoint_slug,
        profile: profile.name.clone(),
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
        preview_url: None,
        files: serde_json::Value::Object(serde_json::Map::new()),
        steps: 0,
        tokens_used: 0,
        created_by: user.0.identity.username.clone(),
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
            &ticket,
            &profile,
            &settings.proxy_base,
            settings.run_ttl_secs,
        );
        return Ok((StatusCode::ACCEPTED, Json(CreatedRun { run, ticket: None })));
    }

    // The workspace is where the ticket goes. Scheduling it is the last step, so a run that
    // could not be recorded never has a pod: the pod is what spends money.
    match kube::schedule_workspace_job(
        state.kube.as_deref(),
        &settings.namespace,
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
        ("limit" = Option<i64>, Query, description = "How many runs to return, at most 100"),
    ),
    responses(
        (status = 200, description = "The project's runs, newest first", body = RunList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 503, description = "The run store did not answer", body = ProblemDetails)
    )
)]
pub async fn list_runs(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(query): Query<ListQuery>,
) -> Result<Json<RunList>, ApiError> {
    let limit = match query.limit {
        Some(n) if n <= 0 => return Err(ApiError::BadRequest("limit must be positive".into())),
        Some(n) => n.min(MAX_LIST_LIMIT),
        None => DEFAULT_LIST_LIMIT,
    };
    let items = state
        .agents
        .list_runs(&project, limit)
        .await
        .map_err(unavailable)?;
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
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<AgentRun>, ApiError> {
    Ok(Json(run_of(&state, &project, &id).await?))
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
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    run_of(&state, &project, &id).await?;
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
        (status = 409, description = "The run is over", body = ProblemDetails)
    )
)]
pub async fn answer_question(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    Json(request): Json<AnswerRequest>,
) -> Result<StatusCode, ApiError> {
    let run = run_of(&state, &project, &id).await?;
    if terminal(&run) {
        return Err(ApiError::Conflict(format!(
            "run '{id}' is '{}' and asks nothing",
            run.status
        )));
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
    Ok(StatusCode::NO_CONTENT)
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
    let run = run_of(&state, &project, &id).await?;
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
    let run = run_of(&state, &project, &id).await?;
    crate::permissions::for_request(&state, &user.0.identity, &project).check(
        "App",
        jc_core::kinds::Verb::Propose,
        None,
    )?;

    let cancelled = state
        .agents
        .set_status(
            &id,
            AgentRunStatus::Cancelled,
            Some(&format!("cancelled by {}", user.0.identity.username)),
        )
        .await
        .map_err(status_error)?;

    // The ticket first, the pod second: between the two calls the workspace is already
    // refused by the proxy, where the other order would leave a live credential for a moment
    // after the user asked for it to stop (AG-46).
    state
        .agents
        .invalidate_ticket(&id)
        .await
        .map_err(unavailable)?;
    if let Some(settings) = state.config.agent_settings.as_ref() {
        kube::delete_workspace_job(state.kube.as_deref(), &settings.namespace, &run.id).await;
    }
    publish_event(
        &state,
        &id,
        "status",
        status_payload(AgentRunStatus::Cancelled),
    )
    .await?;
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
    let run = run_of(&state, &project, &id).await?;
    let status = AgentRunStatus::parse(&run.status);
    if status != Some(AgentRunStatus::Previewing) {
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
    let response = crate::api::mutate::propose(
        &user,
        &state,
        &project,
        "apps",
        Some(&run.app_name),
        operation,
        false,
        manifest,
    )
    .await?;

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
    authenticate_proxy(&state, &headers)?;
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .ok_or_else(|| ApiError::NotFound(format!("run '{id}' not found")))?;
    let profile = Profile::load(&state.mirror, &run.profile)?;
    Ok(Json(RunContext {
        id: run.id,
        project: run.project,
        app_name: run.app_name,
        endpoint_slug: run.endpoint_slug,
        allows_write: run.allows_write,
        branch: run.branch,
        path_prefix: run.path_prefix,
        status: run.status,
        ticket_hash: run.ticket_hash,
        max_tokens: profile.max_tokens_per_run,
        allowed_hosts: profile.allowed_hosts,
        requests_per_minute: profile.requests_per_minute,
        max_response_bytes: profile.max_response_bytes,
        created_by: run.created_by,
        model_name: profile.model_name,
    }))
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
    authenticate_proxy(&state, &headers)?;
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

pub async fn internal_post_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(relayed): Json<RelayedEvent>,
) -> Result<(StatusCode, Json<EventReceipt>), ApiError> {
    authenticate_proxy(&state, &headers)?;
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

/// The bearer the proxy presents. Compared in constant time, and an unconfigured agent runner
/// refuses rather than accepts: there is no token to match, so nothing may call this.
fn authenticate_proxy(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let settings = state
        .config
        .agent_settings
        .as_ref()
        .ok_or(ApiError::Unauthorized)?;
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;
    if crate::auth::csrf::constant_time_eq(settings.proxy_token(), presented) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
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

/// Records one event and hands it to every connected stream.
async fn publish_event(
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

fn status_payload(status: AgentRunStatus) -> serde_json::Value {
    serde_json::json!({ "status": status.as_str(), "timestamp": now_rfc3339() })
}

fn terminal(run: &AgentRun) -> bool {
    AgentRunStatus::parse(&run.status).is_some_and(|status| status.is_terminal())
}

fn agent_settings(state: &AppState) -> Result<&AgentSettings, ApiError> {
    state.config.agent_settings.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no agent runner configured (AG-33)".into())
    })
}

/// The slug the gateway addresses the run's endpoint by (EP-02). It is in the manifest, so an
/// endpoint that has none is a configuration error rather than a run with nothing to read.
fn endpoint_slug(state: &AppState, project: &str, endpoint: &str) -> Result<String, ApiError> {
    state
        .mirror
        .get(project, "Endpoint", endpoint)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "endpoint '{endpoint}' not found in project '{project}'"
            ))
        })?
        .spec
        .get("slug")
        .and_then(serde_json::Value::as_str)
        .filter(|slug| !slug.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ApiError::BadRequest(format!("endpoint '{endpoint}' has no slug (EP-02)")))
}

fn expiry(ttl_secs: i64) -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::seconds(ttl_secs))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn unavailable(err: StoreError) -> ApiError {
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
        (status = 200, description = "The kit rendering the run's specification, one document", content_type = "text/html"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such run, or no pass has written spec.json yet", body = ProblemDetails),
        (status = 503, description = "This Portal was built without the kit", body = ProblemDetails)
    )
)]
/// The preview of a kit run: the bundle, the specification and the stylesheet in one document,
/// because the frame it is shown in has no origin to fetch anything else with (AP-60, UI-41).
pub async fn preview(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let run = state
        .agents
        .get_run(&id)
        .await
        .map_err(unavailable)?
        .filter(|run| run.project == project)
        .ok_or_else(|| {
            ApiError::NotFound(format!("run '{id}' not found in project '{project}'"))
        })?;
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
    let (js, css) = kit::bundle().ok_or_else(|| {
        ApiError::Unavailable("this Portal was built without the kit (kit/dist is empty)".into())
    })?;
    let data = run
        .files
        .get(kit::DATA_FILE)
        .and_then(serde_json::Value::as_str)
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok());
    let html = kit::document(
        &spec.title,
        &run.endpoint_slug,
        &spec,
        data.as_ref(),
        &js,
        &css,
    );
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
    let csp = kit::content_security_policy(&origin, &kit::script_hash(&js));
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

/// The preview route alone, merged beside the Portal's own routes rather than under them: the
/// document carries its own policy, and the Portal's `script-src 'self'` would override it.
pub fn preview_router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/projects/{project}/agent-runs/{id}/preview",
        get(preview),
    )
}

/// The two routes the credential proxy calls, served on the internal listener alone (AG-52).
pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/internal/agent-runs/events", post(internal_post_event))
        .route("/internal/agent-runs/{id}", get(internal_get_run))
        .route("/internal/agent-runs/{id}/inbox", get(internal_inbox))
}
