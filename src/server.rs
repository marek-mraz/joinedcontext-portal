use axum::extract::Request;
use axum::http::{header, HeaderName, HeaderValue};
use axum::middleware::{from_fn, from_fn_with_state, Next};
use axum::response::Response;
use axum::Router;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::api;
use crate::apps;
use crate::assets;
use crate::auth;
use crate::config::Config;
use crate::openapi;
use crate::state::AppState;
use crate::telemetry;

pub fn app(state: AppState) -> Router {
    // The recorder belongs to the surface rather than to `main`: without it every `metrics::`
    // call in the process is a no-op, and a Portal that counted nothing would look exactly
    // like one nobody used (OPS-16).
    telemetry::install();

    let content_security_policy = HeaderValue::from_static(
        "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self'; \
         form-action 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; \
         font-src 'self' data:; connect-src 'self'; worker-src 'self' blob:",
    );

    // A static app carries its own Content Security Policy and its own framing rule, built from
    // its manifest (AP-12); the Portal's would override them, so those two headers are set on
    // the Portal's own routes only. Everything below them applies to both.
    let portal = Router::new()
        .nest("/api/v1", api::router())
        .merge(openapi::router())
        .fallback(assets::static_handler)
        // Every portal request, API or page, keeps the cookie session alive against the realm;
        // outermost of the portal's own layers so a rotated cookie reaches every response.
        .layer(from_fn_with_state(state.clone(), auth::refresh::middleware))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("SAMEORIGIN"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            content_security_policy,
        ));

    Router::new()
        .merge(apps::static_host::router())
        // OPS-16: what `components/monitoring` scrapes. Outside the Portal's own security
        // headers and outside `/api/v1`, so no session or CSRF guard stands in front of a
        // scrape; the edge refuses the path, which is what keeps it inside the cluster.
        .merge(telemetry::router())
        .merge(portal)
        .with_state(state)
        .layer(from_fn(telemetry::record))
        .layer(from_fn(api_cache_control_middleware))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
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
}

async fn api_cache_control_middleware(request: Request, next: Next) -> Response {
    // The branding block is public and holds no secret, and every page load needs it before
    // the session is known, so it is the one API answer a browser may keep (UI-30).
    let is_api =
        request.uri().path().starts_with("/api/") && request.uri().path() != "/api/v1/branding";
    let mut response = next.run(request).await;
    // An event stream sets its own `no-cache`, which is what the SSE contract asks for; it must
    // not be overwritten with `no-store`, because some intermediaries read that as a reason not
    // to stream at all (AG-45).
    let streaming = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if is_api && !streaming {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// The listener the credential proxy calls back on (AG-52).
///
/// A second listener rather than a path on the first: the edge routes `config.bind` and routes
/// nothing here, so the two calls that carry a run's ticket hash are not on the public URL
/// scheme at all. It carries no session layer, no CSRF guard and no static host — the proxy's
/// bearer is the whole of its authentication, checked inside the handlers.
pub fn internal_app(state: AppState) -> Router {
    Router::new()
        .merge(api::agent_runs::internal_router())
        .with_state(state)
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

        // The `SyncSource` loop runs beside it, on the same replica (MF-28, CC-03).
        if let Some(driver) = state.sync.as_ref() {
            crate::sync::driver::spawn_periodic(driver.clone(), syncer.clone());
        }

        // The foreign-model mirror runs beside it, on the same replica (DM-48, DM-49). A
        // Portal whose peers cannot be reached still serves; the mirror says so in its log and
        // the reference stands with whatever the repository already pinned.
        if let Some(gitea) = state.gitea.as_ref() {
            match crate::sync::schema_api::PeerSchemaApi::new() {
                Ok(peers) => {
                    crate::sync::mirror::spawn_periodic(
                        gitea.clone(),
                        syncer.clone(),
                        state.config.public_base_url.to_string(),
                        std::sync::Arc::new(peers),
                    );
                }
                Err(err) => {
                    tracing::warn!(error = %err, "no HTTP client for peer schema surfaces; foreign models are not mirrored")
                }
            }
        }
    }

    // The internal listener runs beside the public one, on the same replica, and only when
    // there is an agent runner to serve: a Portal without one opens no second port.
    if let Some(settings) = state.config.agent_settings.as_ref() {
        let internal = tokio::net::TcpListener::bind(settings.internal_bind).await?;
        tracing::info!(bind = %settings.internal_bind, "portal internal listener listening");
        let internal_router = internal_app(state.clone());
        tokio::spawn(async move {
            if let Err(err) = axum::serve(internal, internal_router)
                .with_graceful_shutdown(shutdown_signal())
                .await
            {
                tracing::error!(error = %err, "the internal listener stopped");
            }
        });
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
