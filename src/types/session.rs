use serde::Serialize;

#[derive(Default, Clone, Serialize)]
pub struct Status {
    pub connected: bool,
    pub logged_in: bool,
    pub username: String,
    pub server: String,
    pub banner: String,
    pub listen_port: u16,
    pub shared_folders: u32,
    pub shared_files: u32,
    pub share_scan_error: Option<String>,
    pub privileges_secs: u32,
    pub peer_connections: usize,
}
