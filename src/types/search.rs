use serde::Serialize;

use crate::types::{FileAttributes, FileInfo};

#[derive(Clone, Serialize)]
pub struct SearchView {
    pub token: u32,
    pub query: String,
    pub results: Vec<SearchResponseView>,
}

#[derive(Clone, Serialize)]
pub struct SearchResponseView {
    pub username: String,
    pub free_upload_slots: bool,
    pub upload_speed: u32,
    pub queue_size: u32,
    pub files: Vec<SearchFileView>,
}

#[derive(Clone, Serialize)]
pub struct SearchFileView {
    pub name: String,
    pub size: u64,
    pub attributes: FileAttributes,
}

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
    pub username: String,
    pub results: Vec<FileInfo>,
    pub free_upload_slots: bool,
    pub upload_speed: u32,
    pub queue_size: u32,
}
