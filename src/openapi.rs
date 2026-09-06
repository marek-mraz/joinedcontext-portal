use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;

use crate::api::health::Health;
use crate::error::ProblemDetails;

#[derive(OpenApi)]
#[openapi(
    paths(crate::api::health::health),
    components(schemas(Health, ProblemDetails)),
    info(
        title = "joinedcontext Portal API",
        version = "0.1.0",
        description = "Administrative and platform management REST API for joinedcontext Portal"
    ),
    tags(
        (name = "system", description = "System operations")
    )
)]
pub struct ApiDoc;

pub async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}

pub fn router() -> Router {
    Router::new().route("/api/v1/openapi.json", get(openapi_json))
}
