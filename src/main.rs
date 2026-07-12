use std::path::PathBuf;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "newkitine=info".into()),
        )
        .init();

    let config_path = PathBuf::from(
        std::env::var("NEWKITINE_CONFIG").unwrap_or_else(|_| "newkitine.toml".into()),
    );
    newkitine::app::run(config_path).await;
}
