use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::api::health::Health;
use crate::auth::Identity;
use crate::error::ProblemDetails;
use crate::state::AppState;

#[derive(OpenApi)]
#[openapi(
    paths(crate::api::health::health, crate::auth::oidc::me),
    components(schemas(Health, Identity, ProblemDetails)),
    info(
        title = "joinedcontext Portal API",
        version = "0.1.0",
        description = "Administrative and platform management REST API for joinedcontext Portal"
    ),
    tags(
        (name = "system", description = "System operations"),
        (name = "auth", description = "Sign-in, sign-out and the current identity")
    )
)]
pub struct ApiDoc;

pub async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/openapi.json", get(openapi_json))
}
