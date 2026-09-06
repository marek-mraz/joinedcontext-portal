use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
pub struct Health {
    pub name: String,
    pub version: String,
    pub status: &'static str,
}

#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "system",
    responses(
        (status = 200, description = "Service is healthy", body = Health)
    )
)]
pub async fn health() -> Json<Health> {
    Json(Health {
        name: crate::APP_NAME.to_string(),
        version: crate::APP_VERSION.to_string(),
        status: "ok",
    })
}

pub fn router() -> axum::Router {
    axum::Router::new().route("/health", axum::routing::get(health))
}
