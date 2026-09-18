//! REST and SSE adapter for shared drafts (AG-61, UI-47, UI-48).
//!
//! Exposes:
//! - `GET /api/v1/projects/{project}/drafts`: list drafts in the project
//! - `GET /api/v1/projects/{project}/drafts/{kind}/{name}`: get a specific draft
//! - `PUT /api/v1/projects/{project}/drafts/{kind}/{name}`: update/create a draft
//! - `DELETE /api/v1/projects/{project}/drafts/{kind}/{name}`: drop a draft
//! - `GET /api/v1/projects/{project}/drafts/events`: SSE stream of draft change events

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use utoipa::ToSchema;

use crate::auth::session::{CurrentUser, Front};
use crate::error::{ApiError, ProblemDetails};
use crate::ops::drafts::{Draft, DraftEvent};
use crate::ops::{self, Caller, Via};
use crate::resource::is_dns1123;
use crate::state::AppState;

/// Input payload for saving or updating a draft manifest.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutDraftRequest {
    pub manifest: Value,
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// Collection of drafts returned by the list operation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DraftList {
    pub items: Vec<Draft>,
}

/// Confirmation payload after dropping a draft.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DropDraftResponse {
    pub dropped: bool,
}

fn caller_of(user: CurrentUser, front: Front) -> Caller {
    Caller {
        identity: user.0.identity,
        via: match front {
            Front::Portal | Front::Edge => Via::Session,
            Front::Bearer => Via::Bearer,
        },
        access: None,
    }
}

fn sse_draft_event(event: &DraftEvent) -> Event {
    let data = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    Event::default()
        .event(event.event.clone())
        .id(event.version.to_string())
        .data(data)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/drafts",
    tag = "drafts",
    params(
        ("project" = String, Path, description = "Project name"),
    ),
    responses(
        (status = 200, description = "List of drafts in the project", body = DraftList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
    )
)]
pub async fn list_drafts(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Response, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let Some(op) = ops::find("jc_draft_list") else {
        return Err(ApiError::NotFound(
            "operation 'jc_draft_list' not found".into(),
        ));
    };
    let caller = caller_of(user, front);
    match ops::call(op, &caller, &state, &project, json!({})).await {
        Ok(output) => Ok((StatusCode::OK, Json(output)).into_response()),
        Err(err) => Ok(err.into_response()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/drafts/{kind}/{name}",
    tag = "drafts",
    params(
        ("project" = String, Path, description = "Project name"),
        ("kind" = String, Path, description = "Manifest kind"),
        ("name" = String, Path, description = "Draft name"),
    ),
    responses(
        (status = 200, description = "The draft", body = Draft),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Draft or project not found", body = ProblemDetails),
    )
)]
pub async fn get_draft(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, kind, name)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let Some(op) = ops::find("jc_draft_get") else {
        return Err(ApiError::NotFound(
            "operation 'jc_draft_get' not found".into(),
        ));
    };
    let caller = caller_of(user, front);
    let input = json!({
        "kind": kind,
        "name": name,
    });
    match ops::call(op, &caller, &state, &project, input).await {
        Ok(output) => Ok((StatusCode::OK, Json(output)).into_response()),
        Err(err) => Ok(err.into_response()),
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/{project}/drafts/{kind}/{name}",
    tag = "drafts",
    params(
        ("project" = String, Path, description = "Project name"),
        ("kind" = String, Path, description = "Manifest kind"),
        ("name" = String, Path, description = "Draft name"),
    ),
    request_body = PutDraftRequest,
    responses(
        (status = 200, description = "The saved draft", body = Draft),
        (status = 400, description = "Bad request, e.g. literal secret", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
        (status = 409, description = "Draft version conflict", body = ProblemDetails),
        (status = 422, description = "Invalid input", body = ProblemDetails),
    )
)]
pub async fn put_draft(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, kind, name)): Path<(String, String, String)>,
    Json(req): Json<PutDraftRequest>,
) -> Result<Response, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let Some(op) = ops::find("jc_draft_put") else {
        return Err(ApiError::NotFound(
            "operation 'jc_draft_put' not found".into(),
        ));
    };
    let caller = caller_of(user, front);
    let mut input = serde_json::Map::new();
    input.insert("kind".to_string(), Value::String(kind));
    input.insert("name".to_string(), Value::String(name));
    input.insert("manifest".to_string(), req.manifest);
    if let Some(expected) = req.expected_version {
        input.insert("expectedVersion".to_string(), json!(expected));
    }
    match ops::call(op, &caller, &state, &project, Value::Object(input)).await {
        Ok(output) => Ok((StatusCode::OK, Json(output)).into_response()),
        Err(err) => Ok(err.into_response()),
    }
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/drafts/{kind}/{name}",
    tag = "drafts",
    params(
        ("project" = String, Path, description = "Project name"),
        ("kind" = String, Path, description = "Manifest kind"),
        ("name" = String, Path, description = "Draft name"),
    ),
    responses(
        (status = 200, description = "Draft dropped result", body = DropDraftResponse),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
    )
)]
pub async fn drop_draft(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, kind, name)): Path<(String, String, String)>,
) -> Result<Response, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let Some(op) = ops::find("jc_draft_drop") else {
        return Err(ApiError::NotFound(
            "operation 'jc_draft_drop' not found".into(),
        ));
    };
    let caller = caller_of(user, front);
    let input = json!({
        "kind": kind,
        "name": name,
    });
    match ops::call(op, &caller, &state, &project, input).await {
        Ok(output) => Ok((StatusCode::OK, Json(output)).into_response()),
        Err(err) => Ok(err.into_response()),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/drafts/events",
    tag = "drafts",
    params(
        ("project" = String, Path, description = "Project name"),
    ),
    responses(
        (status = 200, description = "Server-Sent Events stream of draft events", content_type = "text/event-stream"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Project not found, or not readable by the caller", body = ProblemDetails),
    )
)]
pub async fn stream_draft_events(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Response, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    // The stream is a read, answered like the list it mirrors (PF-59, R20, T-1407).
    if !crate::permissions::for_request(&state, &user.0.identity, &project).may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }

    let live = state.draft_events.subscribe(&project).await;
    let stream = BroadcastStream::new(live).filter_map(
        |item| -> Option<Result<Event, std::convert::Infallible>> {
            match item {
                Ok(event) => Some(Ok(sse_draft_event(&event))),
                Err(BroadcastStreamRecvError::Lagged(_)) => None,
            }
        },
    );

    let sse = Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    );

    let mut response = sse.into_response();
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/drafts", get(list_drafts))
        .route(
            "/projects/{project}/drafts/events",
            get(stream_draft_events),
        )
        .route(
            "/projects/{project}/drafts/{kind}/{name}",
            get(get_draft).put(put_draft).delete(drop_draft),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_draft_request_deserialization() {
        let json_str = r#"{"manifest":{"kind":"DataSource"},"expectedVersion":3}"#;
        let req: PutDraftRequest = serde_json::from_str(json_str).expect("deserialize");
        assert_eq!(req.expected_version, Some(3));
        assert_eq!(req.manifest["kind"], "DataSource");
    }

    #[test]
    fn sse_event_formatting() {
        let event = DraftEvent {
            project: "ovzdusie".into(),
            kind: "DataSource".into(),
            name: "air-sensor".into(),
            version: 2,
            touched_by: "steward".into(),
            touched_kind: "person".into(),
            event: "put".into(),
            updated_at: chrono::Utc::now(),
        };
        let sse = sse_draft_event(&event);
        assert!(format!("{sse:?}").contains("put"));
    }
}
