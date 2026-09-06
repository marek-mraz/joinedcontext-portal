pub mod blueprints;
pub mod branding;
pub mod changes;
pub mod ckan;
pub mod delete;
pub mod dry_run;
pub mod export;
pub mod health;
pub mod mutate;
pub mod pipelines;
pub mod preferences;
pub mod resources;
pub mod service_accounts;
pub mod sync;
pub mod webhook;

use axum::http::Uri;
use axum::response::IntoResponse;
use axum::Router;

use crate::auth;
use crate::error::ApiError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    // Routes protected by double-submit CSRF tokens (session-based UI mutations).
    let protected = Router::new()
        .merge(health::router())
        .merge(auth::oidc::router())
        .merge(blueprints::router())
        .merge(branding::router())
        .merge(changes::router())
        .merge(ckan::router())
        .merge(export::router())
        .merge(resources::router())
        .merge(pipelines::router())
        .merge(preferences::router())
        .merge(service_accounts::router())
        .merge(sync::router())
        .merge(crate::tools::model_tools::router())
        .layer(axum::middleware::from_fn(auth::csrf::require_csrf));

    // The Gitea webhook is a server-to-server call authenticated by its HMAC signature
    // (x-gitea-signature), so it must be exempt from the session guard and CSRF protection.
    // It is merged outside the require_csrf middleware layer as the sole exemption.
    Router::new()
        .merge(webhook::router())
        .merge(protected)
        .fallback(api_not_found)
}

async fn api_not_found(uri: Uri) -> impl IntoResponse {
    ApiError::NotFound(format!("API endpoint '{}' not found", uri.path()))
}
