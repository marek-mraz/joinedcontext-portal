pub mod health;

use axum::http::Uri;
use axum::response::IntoResponse;
use axum::Router;

use crate::error::ApiError;

pub fn router() -> Router {
    Router::new()
        .merge(health::router())
        .fallback(api_not_found)
}

async fn api_not_found(uri: Uri) -> impl IntoResponse {
    ApiError::NotFound(format!("API endpoint '{}' not found", uri.path()))
}
