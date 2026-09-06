pub mod health;
pub mod resources;

use axum::http::Uri;
use axum::response::IntoResponse;
use axum::Router;

use crate::auth;
use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(health::router())
        .merge(auth::oidc::router())
        .merge(resources::router())
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn(auth::csrf::require_csrf))
}

async fn api_not_found(uri: Uri) -> impl IntoResponse {
    ApiError::NotFound(format!("API endpoint '{}' not found", uri.path()))
}
