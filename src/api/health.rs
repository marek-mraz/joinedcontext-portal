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

/// Whether this replica serves the repository yet: `ready` or `loading`, nothing more (OPS-51).
#[derive(Debug, Clone, Serialize, ToSchema, PartialEq, Eq)]
pub struct Readiness {
    pub status: &'static str,
}

#[utoipa::path(
    get,
    path = "/api/v1/ready",
    tag = "system",
    responses(
        (status = 200, description = "The mirror holds the repository", body = Readiness),
        (status = 503, description = "The mirror has not loaded yet", body = Readiness)
    )
)]
pub async fn ready(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
) -> (axum::http::StatusCode, Json<Readiness>) {
    // Without a forge there is no repository to wait for.
    if state.syncer.as_ref().is_none_or(|syncer| syncer.is_ready()) {
        (
            axum::http::StatusCode::OK,
            Json(Readiness { status: "ready" }),
        )
    } else {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(Readiness { status: "loading" }),
        )
    }
}

pub fn router() -> axum::Router<crate::state::AppState> {
    axum::Router::new()
        .route("/health", axum::routing::get(health))
        .route("/ready", axum::routing::get(ready))
}
