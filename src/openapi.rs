use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::api::dry_run::DryRunResult;
use crate::api::health::Health;
use crate::api::resources::{ListMeta, ResourceList};
use crate::auth::oidc::LogoutTarget;
use crate::auth::Identity;
use crate::change::{Change, ChangeMeta, ChangePhase, ChangeStatus, Lane, PlanSummary};
use crate::error::ProblemDetails;
use crate::plan::{FieldChange, PlanDiff};
use crate::resource::{Condition, ObjectMeta, Phase, ResourceEnvelope, Status};
use crate::state::AppState;
use crate::sync::SyncStatus;

#[derive(OpenApi)]
#[openapi(
    paths(
        crate::api::health::health,
        crate::auth::oidc::me,
        crate::auth::oidc::logout,
        crate::api::resources::list,
        crate::api::resources::get_resource,
        crate::api::mutate::create,
        crate::api::mutate::replace,
        crate::api::mutate::patch,
        crate::api::delete::delete_resource,
        crate::api::sync::get_sync_status,
        crate::api::webhook::gitea_webhook,
    ),
    components(schemas(
        Health,
        Identity,
        LogoutTarget,
        ProblemDetails,
        ResourceEnvelope,
        ObjectMeta,
        Status,
        Phase,
        Condition,
        ResourceList,
        ListMeta,
        Change,
        ChangeMeta,
        ChangeStatus,
        ChangePhase,
        Lane,
        PlanSummary,
        PlanDiff,
        FieldChange,
        DryRunResult,
        SyncStatus,
    )),
    info(
        title = "joinedcontext Portal API",
        version = "0.1.0",
        description = "Administrative and platform management REST API for joinedcontext Portal"
    ),
    tags(
        (name = "system", description = "System operations"),
        (name = "auth", description = "Sign-in, sign-out and the current identity"),
        (name = "resources", description = "Resource operations")
    )
)]
pub struct ApiDoc;

pub async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/openapi.json", get(openapi_json))
}
