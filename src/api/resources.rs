use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource::selector::{FieldSelector, LabelSelector};
use crate::resource::{by_plural, ResourceEnvelope, API_VERSION};
use crate::state::AppState;
use crate::store::ListOptions;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceList {
    pub api_version: String,
    pub kind: String,
    pub metadata: ListMeta,
    pub items: Vec<ResourceEnvelope>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListMeta {
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_item_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default, rename = "labelSelector")]
    pub label_selector: Option<String>,
    #[serde(default, rename = "fieldSelector")]
    pub field_selector: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default, rename = "continue")]
    pub continue_token: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/{plural}",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("labelSelector" = Option<String>, Query, description = "Label selector"),
        ("fieldSelector" = Option<String>, Query, description = "Field selector"),
        ("limit" = Option<usize>, Query, description = "Page limit"),
        ("continue" = Option<String>, Query, description = "Pagination continue token"),
        ("revision" = Option<String>, Query, description = "Historical revision"),
    ),
    responses(
        (status = 200, description = "List of resources", body = ResourceList),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Resource not found", body = ProblemDetails)
    )
)]
pub async fn list(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, plural)): Path<(String, String)>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ResourceList>, ApiError> {
    if query.revision.is_some() {
        return Err(ApiError::NotImplemented(
            "historical revisions are served from Git".into(),
        ));
    }

    let kind_info = by_plural(&plural).ok_or_else(|| {
        ApiError::NotFound(format!(
            "plural '{plural}' not found in project '{project}'"
        ))
    })?;

    let limit = match query.limit {
        Some(0) => return Err(ApiError::BadRequest("limit must be greater than 0".into())),
        Some(n) if n > 500 => Some(500),
        Some(n) => Some(n),
        None => None,
    };

    let label_selector = match query.label_selector.as_deref() {
        Some(raw) if !raw.trim().is_empty() => {
            Some(LabelSelector::parse(raw).map_err(|e| ApiError::BadRequest(e.to_string()))?)
        }
        _ => None,
    };

    let field_selector = match query.field_selector.as_deref() {
        Some(raw) if !raw.trim().is_empty() => {
            Some(FieldSelector::parse(raw).map_err(|e| ApiError::BadRequest(e.to_string()))?)
        }
        _ => None,
    };

    let opts = ListOptions {
        label_selector,
        field_selector,
        limit,
        continue_token: query.continue_token,
    };

    let page = state.mirror.list(&project, kind_info.kind, &opts);

    let remaining_item_count = if page.continue_token.is_some() {
        Some(page.remaining)
    } else {
        None
    };

    Ok(Json(ResourceList {
        api_version: API_VERSION.to_string(),
        kind: "List".to_string(),
        metadata: ListMeta {
            continue_token: page.continue_token,
            remaining_item_count,
        },
        items: page.items,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/{plural}/{name}",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("plural" = String, Path, description = "Resource kind plural"),
        ("name" = String, Path, description = "Resource name"),
    ),
    responses(
        (status = 200, description = "Resource envelope", body = ResourceEnvelope),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Resource not found", body = ProblemDetails)
    )
)]
pub async fn get_resource(
    _user: CurrentUser,
    State(state): State<AppState>,
    Path((project, plural, name)): Path<(String, String, String)>,
) -> Result<Json<ResourceEnvelope>, ApiError> {
    // The same 404 body for an unknown plural and for a resource that exists but is not
    // visible: existence is never disclosed (R20).
    let not_found = || {
        ApiError::NotFound(format!(
            "resource '{name}' not found in project '{project}'"
        ))
    };
    let kind_info = by_plural(&plural).ok_or_else(not_found)?;
    let envelope = state
        .mirror
        .get(&project, kind_info.kind, &name)
        .ok_or_else(not_found)?;

    Ok(Json(envelope))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/{plural}", get(list))
        .route("/projects/{project}/{plural}/{name}", get(get_resource))
}
