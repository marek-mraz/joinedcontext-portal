//! The app process: read the variables the reconciler sets, bind loopback, poll, serve.
//!
//! The bind address is loopback because the oauth2-proxy sidecar is the only entrance to the
//! pod (AP-26). Nothing here opens a port for anybody else, and the container declares none.
//! On a public app the sidecar passes anonymous requests straight through (AP-28).

use std::sync::Arc;

use hsl_transport::{router, App, Config};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let bind = std::env::var("JC_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    tracing::info!(%bind, base = %config.base_path, poll = config.poll_seconds, "hsl-transport starting");

    let app = Arc::new(App::new(config));
    // One poll loop for the whole process, however many browsers are watching.
    tokio::spawn(Arc::clone(&app).run());

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
