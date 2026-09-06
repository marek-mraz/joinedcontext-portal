use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Configuration error: {err}");
            std::process::exit(1);
        }
    };

    if let Err(err) = server::serve(config).await {
        eprintln!("Server error: {err}");
        std::process::exit(1);
    }
}
