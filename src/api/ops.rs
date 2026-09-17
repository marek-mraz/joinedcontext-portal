//! REST adapter for the operation registry (AG-59, ADR-N-021, API/01 §20).
//!
//! Exposes:
//! - `GET /api/v1/projects/{project}/ops`: list operations available to the caller
//! - `POST /api/v1/projects/{project}/ops/{name}`: execute an operation by name

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;

use crate::auth::session::{CurrentUser, Front};
use crate::error::{ApiError, ProblemDetails};
use crate::ops::{self, Caller, Via};
use crate::resource::is_dns1123;
use crate::state::AppState;

pub use crate::ops::{OperationAnnotations, OperationSummary};

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/ops",
    tag = "ops",
    params(
        ("project" = String, Path, description = "Project slug"),
    ),
    responses(
        (status = 200, description = "List of operations available to caller", body = Vec<OperationSummary>),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
    )
)]
pub async fn list_ops(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<Vec<OperationSummary>>, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let caller = Caller {
        identity: user.0.identity,
        via: match front {
            Front::Portal => Via::Session,
            Front::Edge => Via::Session,
            Front::Bearer => Via::Bearer,
        },
        access: None,
    };
    Ok(Json(ops::listing(&caller, &state, &project)))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/ops/{name}",
    tag = "ops",
    params(
        ("project" = String, Path, description = "Project slug"),
        ("name" = String, Path, description = "Operation name"),
    ),
    request_body(
        content = Option<Value>,
        description = "Operation input parameters",
        content_type = "application/json"
    ),
    responses(
        (status = 200, description = "Operation executed successfully", body = Object),
        (status = 202, description = "Operation created a change proposal", body = Object),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Operation or project not found", body = ProblemDetails),
        (status = 409, description = "Conflict or verdict required", body = Object),
        (status = 422, description = "Invalid input according to schema", body = ProblemDetails),
    )
)]
pub async fn run_op(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    if !is_dns1123(&project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let Some(op) = ops::find(&name) else {
        return Err(ApiError::NotFound(format!("operation '{name}' not found")));
    };

    let body_val: Value = if body.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?
    };

    let caller = Caller {
        identity: user.0.identity,
        via: match front {
            Front::Portal => Via::Session,
            Front::Edge => Via::Session,
            Front::Bearer => Via::Bearer,
        },
        access: None,
    };

    respond(ops::call(op, &caller, &state, &project, body_val).await)
}

/// The HTTP answer of a registered operation: 202 for a change, 200 otherwise, and a 409 with
/// the operation's own body for a conflict (the strict gate names its check there, PF-57).
pub(crate) fn respond(result: Result<Value, ops::OpError>) -> Result<Response, ApiError> {
    match result {
        Ok(output) => {
            if output.get("changeId").is_some()
                || output.get("change_id").is_some()
                || output.get("kind").and_then(Value::as_str) == Some("Change")
            {
                Ok((StatusCode::ACCEPTED, Json(output)).into_response())
            } else {
                Ok((StatusCode::OK, Json(output)).into_response())
            }
        }
        Err(ops::OpError::Conflict(val)) => Ok((
            StatusCode::CONFLICT,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            Json(val),
        )
            .into_response()),
        Err(err) => Ok(err.into_response()),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/ops", get(list_ops))
        .route("/projects/{project}/ops/{name}", post(run_op))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Lane;
    use serde_json::json;

    #[test]
    fn annotations_camel_case_serialization() {
        let ann = OperationAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
        };
        let val = serde_json::to_value(ann).expect("serialize annotations");
        assert_eq!(val["readOnlyHint"], true);
        assert_eq!(val["destructiveHint"], false);
        assert_eq!(val["idempotentHint"], true);
    }

    #[test]
    fn summary_serialization() {
        let summary = OperationSummary {
            name: "jc_catalog_search".into(),
            title: "Search Catalog".into(),
            description: "Search context resources".into(),
            input_schema: json!({ "type": "object" }),
            output_schema: json!({ "type": "object" }),
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            lane: Lane::Green,
        };
        let val = serde_json::to_value(&summary).expect("serialize summary");
        assert_eq!(val["name"], "jc_catalog_search");
        assert_eq!(val["lane"], "green");
        assert_eq!(val["annotations"]["readOnlyHint"], true);
    }
}
