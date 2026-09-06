use axum::extract::Request;
use axum::http::{header, HeaderName, HeaderValue};
use axum::middleware::{from_fn, Next};
use axum::response::Response;
use axum::Router;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::api;
use crate::assets;
use crate::config::Config;
use crate::openapi;
use crate::state::AppState;

pub fn app(state: AppState) -> Router {
    let content_security_policy = HeaderValue::from_static(
        "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self'; \
         form-action 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; \
         font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:",
    );

    Router::new()
        .nest("/api/v1", api::router())
        .merge(openapi::router())
        .fallback(assets::static_handler)
        .with_state(state)
        .layer(from_fn(api_cache_control_middleware))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("SAMEORIGIN"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("geolocation=(self), camera=(), microphone=()"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            content_security_policy,
        ))
}

async fn api_cache_control_middleware(request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    if is_api {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

pub async fn serve(config: Config) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(bind = %local_addr, public_url = %config.public_base_url, "portal server listening");
    let state = AppState::from_config(config)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    if let Some(syncer) = state.syncer.as_ref() {
        syncer.clone().spawn_periodic(state.config.sync_interval);
    }

    let router = app(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "failed to listen for ctrl+c");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
