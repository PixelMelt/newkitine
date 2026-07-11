use crate::types::FileInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchScope {
    Global,
    Room(String),
    Buddies,
    User(String),
}

#[derive(Debug)]
pub struct SearchResult {
    pub token: u32,
    pub query: String,
    pub username: String,
    pub results: Vec<FileInfo>,
    pub free_upload_slots: bool,
    pub upload_speed: u32,
    pub queue_size: u32,
}
