use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::SharedFolder;

pub const DEFAULT_SERVER: &str = "server.slsknet.org:2242";
pub const DEFAULT_UPLOAD_SLOTS: usize = 2;
pub const DEFAULT_QUEUE_FILE_LIMIT: usize = 500;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeConfig {
    pub login: LoginConfig,
    pub description: String,
    pub auto_reconnect: bool,
    pub transfers: TransferConfig,
    pub shared_folders: Vec<SharedFolder>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoginConfig {
    pub server: SocketAddr,
    pub username: String,
    pub password: String,
    pub listen_port: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransferConfig {
    pub download_dir: PathBuf,
    pub incomplete_dir: PathBuf,
    pub upload_slots: usize,
    pub queue_file_limit: usize,
    pub upload_limit_kbps: u32,
    pub download_limit_kbps: u32,
}

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
    pub filter_level: String,
    pub denied_message: String,
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
            filter_level: "open".into(),
            denied_message: "You need to share files to download from me.".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicSettings {
    pub server: String,
    pub username: String,
    pub password_set: bool,
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
    pub filter_level: String,
    pub denied_message: String,
}

impl From<&Settings> for PublicSettings {
    fn from(settings: &Settings) -> Self {
        Self {
            server: settings.server.clone(),
            username: settings.username.clone(),
            password_set: !settings.password.is_empty(),
            listen_port: settings.listen_port,
            description: settings.description.clone(),
            download_dir: settings.download_dir.clone(),
            incomplete_dir: settings.incomplete_dir.clone(),
            shares: settings.shares.clone(),
            upload_slots: settings.upload_slots,
            queue_file_limit: settings.queue_file_limit,
            upload_limit_kbps: settings.upload_limit_kbps,
            download_limit_kbps: settings.download_limit_kbps,
            auto_reconnect: settings.auto_reconnect,
            theme: settings.theme.clone(),
            filter_level: settings.filter_level.clone(),
            denied_message: settings.denied_message.clone(),
        }
    }
}

impl Settings {
    pub fn locked_differs(&self, other: &Settings, key: &'static str) -> bool {
        match key {
            "server" => self.server != other.server,
            "username" => self.username != other.username,
            "password" => self.password != other.password,
            "listen_port" => self.listen_port != other.listen_port,
            "download_dir" => self.download_dir != other.download_dir,
            key => unreachable!("unknown locked settings key {key}"),
        }
    }

    pub fn copy_locked_from(&mut self, source: &Settings, key: &'static str) {
        match key {
            "server" => self.server = source.server.clone(),
            "username" => self.username = source.username.clone(),
            "password" => self.password = source.password.clone(),
            "listen_port" => self.listen_port = source.listen_port,
            "download_dir" => self.download_dir = source.download_dir.clone(),
            key => unreachable!("unknown locked settings key {key}"),
        }
    }

    pub fn resolve_server(&self) -> Result<std::net::SocketAddr, String> {
        self.server
            .to_socket_addrs()
            .map_err(|error| format!("cannot resolve server address {}: {error}", self.server))?
            .next()
            .ok_or_else(|| format!("cannot resolve server address {}", self.server))
    }

    pub fn incomplete_dir(&self) -> PathBuf {
        self.incomplete_dir
            .clone()
            .unwrap_or_else(|| self.download_dir.join("incomplete"))
    }

    pub fn runtime_config(&self) -> Result<RuntimeConfig, String> {
        Ok(RuntimeConfig {
            login: LoginConfig {
                server: self.resolve_server()?,
                username: self.username.clone(),
                password: self.password.clone(),
                listen_port: self.listen_port,
            },
            description: self.description.clone(),
            auto_reconnect: self.auto_reconnect,
            transfers: TransferConfig {
                download_dir: self.download_dir.clone(),
                incomplete_dir: self.incomplete_dir(),
                upload_slots: self.upload_slots,
                queue_file_limit: self.queue_file_limit,
                upload_limit_kbps: self.upload_limit_kbps,
                download_limit_kbps: self.download_limit_kbps,
            },
            shared_folders: self.shares.clone(),
        })
    }
}
