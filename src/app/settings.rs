use std::net::ToSocketAddrs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use newkitine::client::{
    ClientConfig, DEFAULT_QUEUE_FILE_LIMIT, DEFAULT_SERVER, DEFAULT_UPLOAD_SLOTS, SharedFolder,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub server: String,
    pub username: String,
    pub password: String,
    pub listen_port: u16,
    pub description: String,
    pub download_dir: PathBuf,
    pub incomplete_dir: Option<PathBuf>,
    pub shares: Vec<SharedFolder>,
    pub upload_slots: usize,
    pub queue_file_limit: usize,
    pub upload_limit_kbps: u32,
    pub download_limit_kbps: u32,
    pub auto_reconnect: bool,
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.into(),
            username: String::new(),
            password: String::new(),
            listen_port: 2234,
            description: String::new(),
            download_dir: PathBuf::from("downloads"),
            incomplete_dir: None,
            shares: Vec::new(),
            upload_slots: DEFAULT_UPLOAD_SLOTS,
            queue_file_limit: DEFAULT_QUEUE_FILE_LIMIT,
            upload_limit_kbps: 0,
            download_limit_kbps: 0,
            auto_reconnect: true,
            theme: "dark".into(),
        }
    }
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

impl Settings {
    pub fn apply_env(&mut self) -> Vec<&'static str> {
        let mut locked = Vec::new();
        if let Some(value) = env_var("NEWKITINE_SERVER") {
            self.server = value;
            locked.push("server");
        }
        if let Some(value) = env_var("NEWKITINE_USERNAME") {
            self.username = value;
            locked.push("username");
        }
        if let Some(value) = env_var("NEWKITINE_PASSWORD") {
            self.password = value;
            locked.push("password");
        }
        if let Some(value) = env_var("NEWKITINE_LISTEN_PORT") {
            self.listen_port = value.parse().expect("NEWKITINE_LISTEN_PORT must be a port number");
            locked.push("listen_port");
        }
        if let Some(value) = env_var("NEWKITINE_DOWNLOAD_DIR") {
            self.download_dir = PathBuf::from(value);
            locked.push("download_dir");
        }
        locked
    }

    pub fn resolve_server(&self) -> Result<std::net::SocketAddr, String> {
        self.server
            .to_socket_addrs()
            .map_err(|error| format!("cannot resolve server address {}: {error}", self.server))?
            .next()
            .ok_or_else(|| format!("cannot resolve server address {}", self.server))
    }

    pub fn incomplete_dir(&self) -> PathBuf {
        self.incomplete_dir.clone().unwrap_or_else(|| self.download_dir.join("incomplete"))
    }

    pub fn to_client_config(&self) -> Result<ClientConfig, String> {
        Ok(ClientConfig {
            server: self.resolve_server()?,
            username: self.username.clone(),
            password: self.password.clone(),
            listen_port: self.listen_port,
            incomplete_dir: self.incomplete_dir(),
            download_dir: self.download_dir.clone(),
            description: self.description.clone(),
            shared_folders: self.shares.clone(),
            upload_slots: self.upload_slots,
            queue_file_limit: self.queue_file_limit,
            upload_limit_kbps: self.upload_limit_kbps,
            download_limit_kbps: self.download_limit_kbps,
            auto_reconnect: self.auto_reconnect,
            buddies: Vec::new(),
            banned: Vec::new(),
            ignored: Vec::new(),
            wishlist: Vec::new(),
            liked_interests: Vec::new(),
            hated_interests: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_round_trip() {
        let mut settings = Settings::default();
        settings.username = "someone".into();
        settings.shares.push(SharedFolder {
            virtual_name: "Music".into(),
            path: PathBuf::from("/data/shared"),
            buddy_only: true,
        });
        settings.theme = "catppuccin".into();
        let json = serde_json::to_string(&settings).unwrap();
        let loaded: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(settings, loaded);
    }
}
