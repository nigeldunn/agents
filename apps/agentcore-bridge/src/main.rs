use agentcore_bridge::{AppState, BridgeConfig, build_router};
use aws_config::BehaviorVersion;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "agentcore_bridge=info".into()))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let config = BridgeConfig::from_env()?;
    let aws = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let agentcore = aws_sdk_bedrockagentcore::Client::new(&aws);
    let ddb = aws_sdk_dynamodb::Client::new(&aws);

    let state = AppState::new(&config, agentcore, ddb);

    let listener = tokio::net::TcpListener::bind(config.listen_addr).await?;
    tracing::info!(
        listen_addr = %config.listen_addr,
        runtime_arn = %config.runtime_arn,
        qualifier = %config.qualifier,
        status_table = %config.status_table,
        "agentcore bridge listening"
    );

    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
