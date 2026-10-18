mod api;
mod behavior;
mod boot;
mod chat;
mod config;
mod contract;
mod db;
mod events;
mod geo;
mod gluetun;
mod interests;
mod peer_history;
mod projection;
mod pushover;
mod search;
mod session;
mod settings;
mod state;
mod stats;
mod transfers;
mod users;

use std::path::PathBuf;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "newkitine=info,soulseek=info".into()),
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
    boot::run(config_path).await;
}
