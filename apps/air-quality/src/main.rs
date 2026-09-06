//! The app process: read the four variables the reconciler sets, bind loopback, serve.
//!
//! The bind address is loopback because the oauth2-proxy sidecar is the only entrance to the
//! pod (AP-26). Nothing here opens a port for anybody else, and the container declares none.

use std::sync::Arc;

use air_quality::{router, App, Config};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let bind = std::env::var("JC_BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    tracing::info!(%bind, base = %config.base_path, "air-quality starting");

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let app = router(Arc::new(App::new(config)));
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
