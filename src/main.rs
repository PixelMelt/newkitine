use std::path::PathBuf;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "newkitine=info".into()),
        )
        .init();

    let config_path = match std::env::var("NEWKITINE_CONFIG") {
        Ok(path) => {
            let path = PathBuf::from(path);
            if !path.exists() {
                panic!(
                    "NEWKITINE_CONFIG points to {} which does not exist",
                    path.display()
                );
            }
            path
        }
        Err(_) => PathBuf::from("newkitine.toml"),
    };
    newkitine::app::run(config_path).await;
}
