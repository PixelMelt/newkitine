use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::SharedFolder;

pub const DEFAULT_SERVER: &str = "server.slsknet.org:2242";
pub const DEFAULT_UPLOAD_SLOTS: usize = 2;
pub const DEFAULT_QUEUE_FILE_LIMIT: usize = 500;
pub const DEFAULT_UPLOADS_PER_USER: usize = 1;
pub const DEFAULT_MAX_SEARCH_RESULTS: usize = 300;
pub const DEFAULT_MIN_SEARCH_CHARS: usize = 3;
pub const DEFAULT_MAX_SEARCH_RESPONSES: usize = 500;
pub const DEFAULT_BANNED_MESSAGE: &str = "Banned";

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeConfig {
    pub login: LoginConfig,
    pub description: String,
    pub auto_reconnect: bool,
    pub transfers: TransferConfig,
    pub search: SearchConfig,
    pub shared_folders: Vec<SharedFolder>,
    pub share_filters: Vec<String>,
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
    pub uploads_per_user: usize,
    pub upload_limit_kbps: u32,
    pub download_limit_kbps: u32,
    pub queue_size_limit_mb: u64,
    pub banned_message: String,
    pub download_username_subfolders: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchConfig {
    pub max_search_results: usize,
    pub min_search_chars: usize,
    pub respond_to_searches: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_search_results: DEFAULT_MAX_SEARCH_RESULTS,
            min_search_chars: DEFAULT_MIN_SEARCH_CHARS,
            respond_to_searches: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterLevel {
    Open,
    Guarded,
    Strict,
}
