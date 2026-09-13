use rmcp::{ServiceExt, transport::stdio};
use toolz::ToolzServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let service = ToolzServer::from_environment()?.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
